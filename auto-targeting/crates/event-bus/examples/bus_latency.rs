//! Латентность шины: zenoh vs UDS-базлайн (критерий задачи — обмен
//! сообщениями и производительность на x86 и ARM).
//!
//! Два процесса на одном хосте; RTT/2 ≈ one-way по одним часам хоста.
//!
//! Запуск (два терминала):
//!   cargo run --release -p event-bus --example bus_latency -- server zenoh
//!   cargo run --release -p event-bus --example bus_latency -- client zenoh
//!   # и то же с `uds` вместо `zenoh` (Unix-only базлайн).
//!
//! Размеры: 64 B (телеметрия) / 1 KiB / 8 KiB (пакет детекций).

#[derive(serde::Serialize, serde::Deserialize)]
struct Ping {
    t0_ns: u64,
    #[serde(default)]
    pad: Vec<u8>,
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn report(mode: &str, results: &[(usize, Vec<f64>)]) {
    println!("=== {mode} one-way latency (µs, RTT/2) ===");
    for (size, s) in results {
        let mut v = s.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pick = |q: f64| v[((q / 100.0) * (v.len() as f64 - 1.0)).round() as usize];
        println!(
            "  {:>6} B  p50={:>8.1}  p95={:>8.1}  p99={:>8.1}  (n={})",
            size,
            pick(50.0),
            pick(95.0),
            pick(99.0),
            v.len()
        );
    }
}

// ===================== zenoh (все платформы) =====================

mod zenoh_mode {
    use super::{now_ns, Ping};
    use event_bus::{BusConfig, EventBus};
    use std::time::Duration;

    pub const ENDPOINT: &str = "tcp/127.0.0.1:17447";

    /// Сервер: слушает шину и эхит at/latency → at/latency/echo.
    pub async fn server() -> Result<(), event_bus::BusError> {
        let bus = EventBus::listen(BusConfig {
            endpoint: ENDPOINT.into(),
            ..BusConfig::default()
        })
        .await?;
        eprintln!("[zenoh] listening on {ENDPOINT}");
        let echo_pub = bus.publisher::<Ping>("at/latency/echo").await?;
        let sub = bus.subscriber::<Ping>("at/latency").await?;
        tokio::spawn(async move {
            while let Ok(p) = sub.recv().await {
                let _ = echo_pub.publish(&p).await;
            }
        });
        futures::future::pending::<()>().await;
        #[allow(unreachable_code)]
        Ok(())
    }

    pub async fn client(
        count: usize,
        warmup: usize,
        sizes: &[usize],
    ) -> Result<Vec<(usize, Vec<f64>)>, event_bus::BusError> {
        let bus = EventBus::connect(ENDPOINT).await?;
        let echo = bus.subscriber::<Ping>("at/latency/echo").await?;
        let pub_ = bus.publisher::<Ping>("at/latency").await?;
        // Declare-ы асинхронно распространяются между peer-ами.
        tokio::time::sleep(Duration::from_millis(400)).await;

        let mut out = Vec::new();
        for &size in sizes {
            let mut samples = Vec::with_capacity(count);
            for i in 0..(count + warmup) {
                let ping = Ping {
                    t0_ns: now_ns(),
                    pad: vec![0u8; size.saturating_sub(64)],
                };
                pub_.publish(&ping).await?;
                let pong = echo.recv_timeout(Duration::from_secs(5)).await?;
                if i >= warmup {
                    samples.push((now_ns() - pong.t0_ns) as f64 / 1000.0 / 2.0);
                }
            }
            out.push((size, samples));
        }
        Ok(out)
    }
}

// ===================== UDS-базлайн (Unix) =====================

#[cfg(unix)]
mod uds_mode {
    use super::Ping;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    const SOCK: &str = "/tmp/at-bus-latency.sock";

    fn write_msg<S: Write>(s: &mut S, bytes: &[u8]) -> std::io::Result<()> {
        s.write_all(&(bytes.len() as u32).to_be_bytes())?;
        s.write_all(bytes)
    }

    fn read_msg<R: Read>(s: &mut R) -> std::io::Result<Vec<u8>> {
        let mut len = [0u8; 4];
        s.read_exact(&mut len)?;
        let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
        s.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn server() -> std::io::Result<()> {
        let _ = std::fs::remove_file(SOCK);
        let listener = UnixListener::bind(SOCK)?;
        eprintln!("[uds] listening on {SOCK}");
        for stream in listener.incoming() {
            let mut stream = stream?;
            while let Ok(bytes) = read_msg(&mut stream) {
                let pong = serde_json::to_vec(&serde_json::from_slice::<Ping>(&bytes).unwrap())?;
                write_msg(&mut stream, &pong)?;
            }
        }
        Ok(())
    }

    pub fn client(count: usize, warmup: usize, sizes: &[usize]) -> Vec<(usize, Vec<f64>)> {
        let mut stream = UnixStream::connect(SOCK).expect("connect uds server");
        let mut out = Vec::new();
        for &size in sizes {
            let mut samples = Vec::with_capacity(count);
            for i in 0..(count + warmup) {
                let ping = Ping {
                    t0_ns: super::now_ns(),
                    pad: vec![0u8; size.saturating_sub(64)],
                };
                let bytes = serde_json::to_vec(&ping).unwrap();
                write_msg(&mut stream, &bytes).unwrap();
                let resp = read_msg(&mut stream).unwrap();
                let pong: Ping = serde_json::from_slice(&resp).unwrap();
                if i >= warmup {
                    samples.push((super::now_ns() - pong.t0_ns) as f64 / 1000.0 / 2.0);
                }
            }
            out.push((size, samples));
        }
        out
    }
}

// ===================== main =====================

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let role = args.get(1).map(String::as_str).unwrap_or("client");
    let mode = args.get(2).map(String::as_str).unwrap_or("zenoh");
    let count: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(300);
    const SIZES: [usize; 3] = [64, 1024, 8192];

    match (role, mode) {
        ("server", "zenoh") => {
            zenoh_mode::server().await.unwrap();
        }
        #[cfg(unix)]
        ("server", "uds") => {
            uds_mode::server().unwrap();
        }
        ("client", "zenoh") => {
            let res = zenoh_mode::client(count, 50, &SIZES).await.unwrap();
            report("zenoh (tcp/lo, peer, JSON)", &res);
        }
        #[cfg(unix)]
        ("client", "uds") => {
            let res = uds_mode::client(count, 50, &SIZES);
            report("uds baseline (unix socket, JSON)", &res);
        }
        _ => {
            eprintln!("usage: bus_latency <server|client> <zenoh|uds> [count]");
            std::process::exit(2);
        }
    }
}
