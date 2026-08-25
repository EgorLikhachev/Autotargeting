//! track_gen — генератор синтетических треков на at/tracks (M4-демо).
//! Издатель роли «трекер» для замкнутого контура без реальной камеры.
//!
//!   track_gen --bus tcp/127.0.0.1:7447 --seconds 20 --offset-x 60 --offset-y 40

use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    bus: String,
    #[arg(long, default_value_t = 20)]
    seconds: u64,
    /// offset центра цели от центра кадра (пиксели).
    #[arg(long, default_value_t = 60.0)]
    offset_x: f32,
    #[arg(long, default_value_t = 40.0)]
    offset_y: f32,
    /// Период публикации, мс.
    #[arg(long, default_value_t = 100)]
    period_ms: u64,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args = Args::parse();
    let bus = event_bus::EventBus::connect(&args.bus)
        .await
        .expect("bus connect");
    let pub_ = bus
        .publisher::<event_bus::TrackMsg>(event_bus::topics::TRACKS)
        .await
        .expect("tracks publisher");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(args.seconds);
    let mut seq: u64 = 0;
    while std::time::Instant::now() < deadline {
        seq += 1;
        // Лёгкое движение цели вокруг заданной точки.
        let cx = 320.0 + args.offset_x + (seq as f32 * 0.5).sin() * 8.0;
        let cy = 240.0 + args.offset_y + (seq as f32 * 0.3).cos() * 6.0;
        let msg = event_bus::TrackMsg {
            v: event_bus::CONTRACT_VERSION,
            track_id: 1,
            frame_seq: seq,
            bbox: common::BoundingBox {
                x: (cx - 25.0) as u32,
                y: (cy - 30.0) as u32,
                width: 50,
                height: 60,
            },
            vx: 0.0,
            vy: 0.0,
            class: "person".into(),
            class_id: 0,
            confidence: 0.9,
            age: seq as u32,
            misses: 0,
        };
        pub_.publish(&msg).await.expect("publish");
        tokio::time::sleep(std::time::Duration::from_millis(args.period_ms)).await;
    }
    println!("[summary] published={seq}");
}
