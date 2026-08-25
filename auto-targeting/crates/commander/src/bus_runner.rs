//! bus_runner — M4: commander на шине (BUS_MIGRATION_PLAN).
//!
//! Подписки: `at/tracks` (TrackMsg от трекера), `at/telemetry`
//! (TelemetrySample от fc-bridge). Публикация: `at/status/commander`.
//!
//! Петля: трек → вычислить offset выбранной цели от центра кадра →
//! `Commander::update(&[Detection], Some(offset))` (анти-loop/watchdogs/
//! rate-limiter — вся существующая safety-логика без изменений) →
//! коррекция уходит в FC через `FlightControllerAdapter` (владение
//! адаптером у commander, как и раньше — шину команды FC не заменяет,
//! это транспорт управляющего контура, не M3-диспетчер команд).
//!
//! Согласование размеров кадра: commander не читает кадры — нормировку
//! bbox-центра делает трекер в `frame_w/h` координатах детекций;
//! для offset-вычисления нужен центр кадра (frame_w/2, frame_h/2) —
//! commander хранит последнюю геометрию из DetectionsFrame... трекер
//! публикует только TrackMsg (без геометрии кадра), поэтому центр
//! приходит в `CommanderBusConfig.frame_center` (по умолчанию 640×480,
//! уточняется из первого трека с флагом geometry).

use std::time::{Duration, Instant};

use event_bus::{topics, EventBus, TrackMsg, CONTRACT_VERSION};

use crate::commander::Commander;

/// Конфигурация bus-режима commander.
#[derive(Debug, Clone)]
pub struct CommanderBusConfig {
    /// Центр кадра (X, Y) для offset-вычислений (пиксели).
    pub frame_center: (f32, f32),
    /// Максимальный возраст трека (кадры misses) для управления.
    pub max_track_misses: u32,
    /// Период статуса на шину.
    pub status_interval: Duration,
    /// Прекратить после N секунд (None — бессрочно).
    pub max_duration: Option<Duration>,
}

impl Default for CommanderBusConfig {
    fn default() -> Self {
        Self {
            frame_center: (320.0, 240.0),
            max_track_misses: 15,
            status_interval: Duration::from_secs(1),
            max_duration: None,
        }
    }
}

/// Статус commander на шине.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommanderStatus {
    pub v: u8,
    pub state: String,
    pub active_target: Option<u64>,
    pub tracks_received: u64,
    pub telemetry_received: u64,
    pub corrections_sent: u64,
    pub corrections_suppressed: u64,
}

/// Итог прогона.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommanderBusStats {
    pub tracks_received: u64,
    pub telemetry_received: u64,
    pub corrections_sent: u64,
    pub corrections_suppressed: u64,
}

/// Bus-контур commander.
pub struct CommanderBus {
    cfg: CommanderBusConfig,
}

impl CommanderBus {
    #[must_use]
    pub fn new(cfg: CommanderBusConfig) -> Self {
        Self { cfg }
    }

    /// Основной цикл. Commander уже создан (с адаптером и конфигом);
    /// connect() внутри (переход Idle→Armed по контракту commander).
    pub async fn run(
        &self,
        commander: &mut Commander,
        bus: &EventBus,
    ) -> Result<CommanderBusStats, event_bus::BusError> {
        commander
            .connect()
            .await
            .map_err(|e| event_bus::BusError::Zenoh(e.to_string()))?;
        let _ = commander.start_scanning();

        let tracks = bus.subscribe_tracks().await?;
        let tele = bus.subscribe_telemetry().await?;
        let status_pub = bus
            .publisher::<CommanderStatus>(&topics::status("commander"))
            .await?;
        tokio::time::sleep(Duration::from_millis(300)).await; // declare-распространение

        let mut stats = CommanderBusStats::default();
        let started = Instant::now();
        let mut last_status = Instant::now();

        loop {
            if let Some(max) = self.cfg.max_duration {
                if started.elapsed() >= max {
                    break;
                }
            }

            // Телеметрия: кормим FcHeartbeat через состояние адаптера
            // (commander сам проверяет в process_watchdog_expiries;
            // здесь — только счётчик и feed при живом heartbeat).
            if let Ok(t) = tele.recv_timeout(Duration::from_millis(0)).await {
                stats.telemetry_received += 1;
                // heartbeat жив, если телеметрия свежая — кормим watchdog.
                commander.watchdog_registry().feed(crate::watchdogs::WatchdogId::FcHeartbeat);
                let _ = t; // поле t_ms — метка источника
            }

            // Треки → петля управления.
            while let Ok(track) = tracks.recv_timeout(Duration::from_millis(0)).await {
                stats.tracks_received += 1;

                // Выбор/подтверждение цели.
                if commander.health_snapshot().active_target_id != Some(track.track_id) {
                    // Первая увиденная цель становится активной (Phase 5
                    // семантика select_target; переключение — отдельная
                    // команда оператора через at/commands в M5).
                    if commander.health_snapshot().active_target_id.is_none()
                        && commander.select_target(track.track_id).is_err() {
                            tracing::debug!(track.track_id, "select_target rejected (state)");
                        }
                }

                // Управление только по свежему треку.
                if track.misses <= self.cfg.max_track_misses
                    && commander.health_snapshot().active_target_id == Some(track.track_id)
                {
                    let (cx, cy) = self.bbox_center(&track);
                    let offset = (cx - self.cfg.frame_center.0, cy - self.cfg.frame_center.1);
                    let before = commander.health_snapshot().rate_limiter_sent;
                    let res = commander
                        .update(&[], Some(offset))
                        .await;
                    if res.is_ok() {
                        let after = commander.health_snapshot().rate_limiter_sent;
                        if after > before {
                            stats.corrections_sent = after;
                        } else {
                            stats.corrections_suppressed += 1;
                        }
                    }
                }

                // Кормим TrackingLoop на каждом треке.
                commander
                    .watchdog_registry()
                    .feed(crate::watchdogs::WatchdogId::TrackingLoop);
                commander
                    .watchdog_registry()
                    .feed(crate::watchdogs::WatchdogId::InferenceLoop);
                commander.feed_video_watchdog();
            }

            // Watchdog-экспирации (Degrade обрабатывается внутри).
            let _expired = commander.process_watchdog_expiries();

            // Статус.
            if last_status.elapsed() >= self.cfg.status_interval {
                let h = commander.health_snapshot();
                let st = CommanderStatus {
                    v: CONTRACT_VERSION,
                    state: format!("{:?}", commander.state()),
                    active_target: commander.health_snapshot().active_target_id,
                    tracks_received: stats.tracks_received,
                    telemetry_received: stats.telemetry_received,
                    corrections_sent: stats.corrections_sent,
                    corrections_suppressed: stats.corrections_suppressed,
                };
                let _ = status_pub.publish(&st).await;
                let _ = h;
                last_status = Instant::now();
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(stats)
    }

    fn bbox_center(&self, t: &TrackMsg) -> (f32, f32) {
        let b = &t.bbox;
        (b.x as f32 + b.width as f32 / 2.0, b.y as f32 + b.height as f32 / 2.0)
    }
}
