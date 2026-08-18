//! bus_dump — подписчик-наблюдатель шины (M1): печатает все сообщения
//! по маске тем (по умолчанию `at/**`) как `topic json`.
//!
//! Инструмент приёмки M1 и дашборд-заготовка:
//!   cargo run --release -p event-bus --example bus_dump -- --listen
//!   # (в другом терминале поднять компоненты с --bus tcp/127.0.0.1:7447)

use clap::Parser;

#[derive(Parser)]
struct Args {
    /// Маска тем zenoh.
    #[arg(long, default_value = "at/**")]
    topics: String,
    /// Роль: поднять listener (первый процесс шины).
    #[arg(long)]
    listen: bool,
    /// Endpoint шины.
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    bus: String,
    /// Свернуть длинные payload (детекции) до N символов.
    #[arg(long, default_value_t = 400)]
    max_len: usize,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args = Args::parse();
    let cfg = event_bus::BusConfig {
        endpoint: args.bus.clone(),
        listen: args.listen,
        scope: String::new(),
    };
    let bus = match event_bus::EventBus::listen(cfg).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[!] bus listen failed: {e}");
            std::process::exit(3);
        }
    };
    let session = bus.session();

    let sub = session
        .declare_subscriber(&args.topics)
        .await
        .expect("declare subscriber");
    eprintln!("[bus_dump] watching {} on {}", args.topics, args.bus);

    use futures::StreamExt;
    let mut stream = sub.stream();
    while let Some(sample) = stream.next().await {
        let topic = sample.key_expr().as_str();
        let payload = sample.payload().to_bytes();
        let mut line = String::from_utf8_lossy(payload.as_ref()).into_owned();
        if line.len() > args.max_len {
            line.truncate(args.max_len);
            line.push_str("...");
        }
        println!("{topic} {line}");
    }
}
