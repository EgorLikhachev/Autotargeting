//! bus_console — M5: операторская консоль на шине.
//!
//! Три режима (главный бинарь):
//! - `bus-mon`  — монитор `at/**` с фильтром и pretty-print;
//! - `repl-bus` — интерактив: команды FC через `at/commands`, живые
//!   tracks/telemetry/statuses из шины;
//! - `config-*` — конфиг-сервис (queryable `at/config`) и `config-get`.
//!
//! rustyline здесь не используется: stdin — через `spawn_blocking` канал
//! (стрелки-редактирование не критично для оператора; зато реактор живой).

use std::collections::HashMap;
use std::sync::Arc;

use common::AppConfig;
use event_bus::{topics, CommandMsg, EventBus, TelemetrySample, TrackMsg, CONTRACT_VERSION};
use parking_lot::Mutex;

/// Подключиться (или поднять listener) по AppConfig.bus.
pub async fn connect_bus(cfg: &AppConfig, listen: bool) -> anyhow::Result<EventBus> {
    let bcfg = event_bus::BusConfig {
        endpoint: cfg.bus.endpoint.clone(),
        listen,
        scope: String::new(),
    };
    if listen {
        EventBus::listen(bcfg).await.map_err(|e| anyhow::anyhow!(e))
    } else {
        EventBus::connect(&cfg.bus.endpoint)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

// ===================== bus-mon =====================

/// Монитор шины: печатает `topic json` по маске (по умолчанию at/**).
pub async fn run_monitor(bus: &EventBus, mask: &str, max_len: usize) -> anyhow::Result<()> {
    let session = bus.session();
    let sub = session
        .declare_subscriber(mask)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    eprintln!("[bus-mon] watching {mask}");
    use futures::StreamExt;
    let mut stream = sub.stream();
    while let Some(sample) = stream.next().await {
        let topic = sample.key_expr().as_str();
        let payload = String::from_utf8_lossy(sample.payload().to_bytes().as_ref()).into_owned();
        let mut line = payload;
        if line.len() > max_len {
            line.truncate(max_len);
            line.push_str("...");
        }
        println!("{topic} {line}");
    }
    Ok(())
}

// ===================== repl-bus =====================

/// Кэш последних сообщений шины для отображения оператору.
#[derive(Default)]
pub struct BusCache {
    pub track: Option<TrackMsg>,
    pub telemetry: Option<TelemetrySample>,
    pub statuses: HashMap<String, String>,
    pub commands_sent: u64,
}

/// Отправить команду FC через шину (target="fc"; диспетчер — fc-bridge).
pub async fn send_fc_command(
    bus: &EventBus,
    cmd: &str,
    args: serde_json::Value,
) -> Result<(), event_bus::BusError> {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let msg = CommandMsg {
        v: CONTRACT_VERSION,
        target: "fc".into(),
        cmd: cmd.into(),
        args,
        source: "cli".into(),
        id,
    };
    bus.publish_commands().await?.publish(&msg).await
}

/// REPL на шине. Блокирующий stdin — в spawn_blocking, команды — из async.
pub async fn run_repl(bus: &EventBus) -> anyhow::Result<()> {
    // Подписки -> кэш (фоновая задача).
    let cache = Arc::new(Mutex::new(BusCache::default()));
    let track_sub = bus.subscribe_tracks().await.map_err(|e| anyhow::anyhow!(e))?;
    let tele_sub = bus
        .subscribe_telemetry()
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let status_sub = bus
        .session()
        .declare_subscriber("at/status/**")
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let cache_writer = Arc::clone(&cache);
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut st = status_sub.stream();
        loop {
            tokio::select! {
                t = track_sub.recv() => { if let Ok(t) = t { cache_writer.lock().track = Some(t); } }
                te = tele_sub.recv() => { if let Ok(te) = te { cache_writer.lock().telemetry = Some(te); } }
                Some(s) = st.next() => {
                    let k = s.key_expr().as_str().to_string();
                    let v = String::from_utf8_lossy(s.payload().to_bytes().as_ref()).into_owned();
                    cache_writer.lock().statuses.insert(k, v);
                }
            }
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await; // declare-распространение

    // stdin -> канал.
    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(8);
    tokio::task::spawn_blocking(move || {
        let mut buf = String::new();
        loop {
            buf.clear();
            match std::io::stdin().read_line(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line_tx.blocking_send(buf.trim().to_string()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    println!("auto-targeting bus REPL — команды: help status tracks telemetry statuses arm disarm mode <m> set-roi <lat> <lon> abort quit");
    while let Some(line) = line_rx.recv().await {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        match cmd {
            "help" | "h" | "?" => println!("status tracks telemetry statuses arm disarm mode <guided|rtl|loiter|auto|manual> set-roi <lat> <lon> <alt> abort quit"),
            "status" | "st" => {
                let c = cache.lock();
                println!("commands_sent: {}", c.commands_sent);
                if let Some(t) = &c.track {
                    println!(
                        "track: id={} bbox=({},{},{}x{}) class={} conf={:.2}",
                        t.track_id, t.bbox.x, t.bbox.y, t.bbox.width, t.bbox.height, t.class, t.confidence
                    );
                } else {
                    println!("track: —");
                }
                if let Some(t) = &c.telemetry {
                    println!(
                        "telemetry: rpy=({:.1},{:.1},{:.1}) alt={:.0} mode={}",
                        t.roll_deg, t.pitch_deg, t.yaw_deg, t.alt_m, t.mode
                    );
                } else {
                    println!("telemetry: —");
                }
                println!("statuses: {} components", c.statuses.len());
            }
            "tracks" => {
                let c = cache.lock();
                match &c.track {
                    Some(t) => println!("{}", serde_json::to_string_pretty(t).unwrap()),
                    None => println!("нет треков"),
                }
            }
            "telemetry" | "tele" => {
                let c = cache.lock();
                match &c.telemetry {
                    Some(t) => println!("{}", serde_json::to_string_pretty(t).unwrap()),
                    None => println!("нет телеметрии"),
                }
            }
            "statuses" => {
                let c = cache.lock();
                if c.statuses.is_empty() {
                    println!("нет статусов");
                }
                for (k, v) in &c.statuses {
                    println!("{k} {v}");
                }
            }
            "arm" | "disarm" => {
                match send_fc_command(bus, cmd, serde_json::json!({})).await {
                    Ok(()) => {
                        cache.lock().commands_sent += 1;
                        println!("-> {cmd} отправлена");
                    }
                    Err(e) => println!("[!] {e}"),
                }
            }
            "mode" | "set-mode" => {
                let m = rest.first().copied().unwrap_or("");
                if m.is_empty() {
                    println!("usage: mode <guided|rtl|loiter|auto|manual|stabilize>");
                } else {
                    match send_fc_command(bus, "set_mode", serde_json::json!({"mode": m})).await {
                        Ok(()) => {
                            cache.lock().commands_sent += 1;
                            println!("-> set_mode {m}");
                        }
                        Err(e) => println!("[!] {e}"),
                    }
                }
            }
            "set-roi" => {
                if rest.len() < 2 {
                    println!("usage: set-roi <lat> <lon> [alt]");
                } else {
                    let (Ok(lat), Ok(lon)) = (rest[0].parse::<f64>(), rest[1].parse::<f64>())
                    else {
                        println!("[!] lat/lon должны быть числами");
                        continue;
                    };
                    let alt: f64 = rest.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                    match send_fc_command(
                        bus,
                        "set_roi",
                        serde_json::json!({"lat": lat, "lon": lon, "alt": alt}),
                    )
                    .await
                    {
                        Ok(()) => {
                            cache.lock().commands_sent += 1;
                            println!("-> set_roi ({lat},{lon},{alt})");
                        }
                        Err(e) => println!("[!] {e}"),
                    }
                }
            }
            "abort" => {
                // Safety: командир слушает команды? Нет — abort дублируется
                // и в fc-bridge (set_mode rtl). Шлём через шину как FC-команду.
                match send_fc_command(bus, "set_mode", serde_json::json!({"mode": "rtl"})).await {
                    Ok(()) => println!("-> ABORT: RTL отправлен"),
                    Err(e) => println!("[!] {e}"),
                }
            }
            "quit" | "exit" | "q" => break,
            "" => {}
            other => println!("неизвестная команда: {other} (help — список)"),
        }
    }
    println!("Bye.");
    Ok(())
}

// ===================== config service =====================

/// Топика запроса конфига (pub/sub-ack паттерн; M5).
pub const CONFIG_GET_TOPIC: &str = "at/config_get";

/// Конфиг-сервис: подписка на запросы `at/config_get`, ответ полным
/// AppConfig на `at/config_ack`. (Zenoh-query в peer-топологии со scouting
/// off капризен; pub/sub-ack проверен всем остальным контуром.)
pub async fn run_config_service(bus: &EventBus, cfg: AppConfig) -> anyhow::Result<()> {
    let req = bus
        .subscriber::<CommandMsg>(CONFIG_GET_TOPIC)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let ack = bus
        .publisher::<serde_json::Value>(topics::CONFIG_ACK)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    eprintln!("[config-svc] answering on {CONFIG_GET_TOPIC} -> {}", topics::CONFIG_ACK);
    let payload = serde_json::to_value(&cfg)?;
    while req.recv().await.is_ok() {
        let _ = ack.publish(&payload).await;
    }
    Ok(())
}

/// Клиент конфига: запрос + ожидание ответа на `at/config_ack`.
pub async fn config_get(bus: &EventBus) -> anyhow::Result<()> {
    let ack = bus
        .subscriber::<serde_json::Value>(topics::CONFIG_ACK)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let req = bus
        .publisher::<CommandMsg>(CONFIG_GET_TOPIC)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await; // declare
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    req.publish(&CommandMsg {
        v: CONTRACT_VERSION,
        target: "config-svc".into(),
        cmd: "get".into(),
        args: serde_json::json!({}),
        source: "cli".into(),
        id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    })
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    match ack.recv_timeout(std::time::Duration::from_secs(5)).await {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
        Err(_) => {
            eprintln!("[!] нет ответа на at/config_ack (config-svc запущен?)");
            std::process::exit(4);
        }
    }
}
