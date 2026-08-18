//! # event-bus — типизированная шина событий/данных между компонентами (Zenoh)
//!
//! Прототип и фундамент выбранной шины (сравнение вариантов —
//! [docs/BUS_SELECTION.md](../../../docs/BUS_SELECTION.md), решение D-014).
//! **Кадры через шину не передаются** — они живут в SHM-кольце `shmem-buffer`;
//! здесь — детекции, телеметрия, команды, конфигурация.
//!
//! Модель: zenoh peer-to-peer **без брокера**. Один из процессов поднимает
//! listener (`EventBus::listen`), остальные подключаются (`EventBus::connect`);
//! multicast-scouting выключен — топология фиксированная (компоненты одного
//! хоста, R9 из требований).
//!
//! ## Темы
//!
//! ```text
//! at/detections   — детекции кадра (JSON, 1–10 КБ)
//! at/telemetry    — телеметрия АП (десятки байт @10–50 Гц)
//! at/commands     — команды оператора/FSM
//! at/config       — конфигурация (query-паттерн — позже)
//! at/status/{c}   — статус компонента c (рекордер и т.д.)
//! ```
//!
//! ## Пример
//!
//! ```no_run
//! # async fn demo() -> Result<(), event_bus::BusError> {
//! let bus = event_bus::EventBus::connect("tcp/127.0.0.1:7447").await?;
//! let mut tele = bus.subscribe_telemetry().await?;
//! let det_pub = bus.publish_detections().await?;
//! let t = tele.recv().await?; // TelemetrySample
//! # let _ = (det_pub, t);
//! # Ok(()) }
//! ```

use std::time::Duration;

use common::Detection;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Ошибки шины.
#[derive(thiserror::Error, Debug)]
pub enum BusError {
    #[error("zenoh: {0}")]
    Zenoh(String),
    #[error("payload serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("subscriber channel closed")]
    Closed,
    #[error("timeout waiting for message")]
    Timeout,
    #[error("invalid endpoint '{0}'")]
    Endpoint(String),
}

/// Темы шины (ключи zenoh).
pub mod topics {
    pub const DETECTIONS: &str = "at/detections";
    pub const TELEMETRY: &str = "at/telemetry";
    pub const COMMANDS: &str = "at/commands";
    pub const CONFIG: &str = "at/config";
    /// Статус компонента: `at/status/{component}`.
    #[must_use]
    pub fn status(component: &str) -> String {
        format!("at/status/{component}")
    }
}

/// Конфигурация шины.
#[derive(Debug, Clone)]
pub struct BusConfig {
    /// Endpoint zenoh (например `tcp/127.0.0.1:7447`).
    pub endpoint: String,
    /// true — поднять listener (первый процесс), false — подключиться.
    pub listen: bool,
    /// Префикс-изоляция сессий (демо/тесты).
    pub scope: String,
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            endpoint: "tcp/127.0.0.1:7447".into(),
            listen: true,
            scope: String::new(),
        }
    }
}

/// Пакет детекций кадра (типичный payload R2: 1–10 КБ).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionsFrame {
    pub frame_seq: u64,
    #[serde(with = "chrono_ts_ms")]
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub detections: Vec<Detection>,
}

/// Телеметрия АП (типичный payload R2: десятки байт).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TelemetrySample {
    pub t_ms: i64,
    pub roll_deg: f32,
    pub pitch_deg: f32,
    pub yaw_deg: f32,
    pub alt_m: f32,
}

mod chrono_ts_ms {
    use chrono::{DateTime, Utc};
    use serde::{self, Deserialize, Deserializer, Serializer};
    pub fn serialize<S>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_i64(dt.timestamp_millis())
    }
    pub fn deserialize<'de, D>(d: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ms = i64::deserialize(d)?;
        Ok(DateTime::from_timestamp_millis(ms).unwrap_or(DateTime::UNIX_EPOCH))
    }
}

/// Соединение с шиной (обёртка над zenoh-сессией).
pub struct EventBus {
    session: zenoh::Session,
    scope: String,
}

impl EventBus {
    fn base_config(cfg: &BusConfig) -> Result<zenoh::config::Config, BusError> {
        use zenoh::config::Config;
        // zenoh 1.10: Config непрозрачен — программная настройка через
        // insert_json5 (документированный путь).
        let mut z = Config::default();
        let insert = |c: &mut Config, k: &str, v: &str| -> Result<(), BusError> {
            c.insert_json5(k, v)
                .map_err(|e| BusError::Endpoint(format!("{k}: {e}")))
        };
        // Фиксированная топология: режим peer, автопоиск (scouting) выключен.
        insert(&mut z, "mode", concat!('"', "peer", '"'))?;
        insert(&mut z, "scouting/multicast/enabled", "false")?;
        // Сначала валидируем endpoint, потом вставляем.
        let _: zenoh::config::EndPoint = cfg.endpoint.parse().map_err(|e| {
            BusError::Endpoint(format!("{}: {e}", cfg.endpoint))
        })?;
        let eps = format!("[\"{}\"]", cfg.endpoint);
        if cfg.listen {
            insert(&mut z, "listen/endpoints", &eps)?;
        } else {
            insert(&mut z, "connect/endpoints", &eps)?;
        }
        Ok(z)
    }

    /// Поднять шину (listener; первый/главный процесс).
    pub async fn listen(cfg: BusConfig) -> Result<Self, BusError> {
        let session = zenoh::open(Self::base_config(&cfg)?)
            .await
            .map_err(zerr)?;
        tracing::info!(endpoint = %cfg.endpoint, "event-bus listening");
        Ok(Self { session, scope: cfg.scope })
    }

    /// Подключиться к шине (остальные процессы).
    pub async fn connect(endpoint: &str) -> Result<Self, BusError> {
        let cfg = BusConfig {
            endpoint: endpoint.to_string(),
            listen: false,
            scope: String::new(),
        };
        let session = zenoh::open(Self::base_config(&cfg)?)
            .await
            .map_err(zerr)?;
        tracing::info!(endpoint, "event-bus connected");
        Ok(Self { session, scope: cfg.scope })
    }

    fn key(&self, topic: &str) -> String {
        if self.scope.is_empty() {
            topic.to_string()
        } else {
            format!("{}/{}", self.scope, topic)
        }
    }

    /// Типизированный издатель на произвольной теме.
    pub async fn publisher<T: Serialize>(
        &self,
        topic: &str,
    ) -> Result<TypedPublisher<T>, BusError> {
        let p = self
            .session
            .declare_publisher(self.key(topic))
            .await
            .map_err(zerr)?;
        Ok(TypedPublisher {
            inner: p,
            _marker: std::marker::PhantomData,
        })
    }

    /// Типизированный подписчик (FIFO).
    pub async fn subscriber<T: DeserializeOwned>(
        &self,
        topic: &str,
    ) -> Result<TypedSubscriber<T>, BusError> {
        let s = self
            .session
            .declare_subscriber(self.key(topic))
            .await
            .map_err(zerr)?;
        Ok(TypedSubscriber {
            inner: s,
            _marker: std::marker::PhantomData,
        })
    }

    // ---- Готовые темы проекта ----

    pub async fn publish_detections(&self) -> Result<TypedPublisher<DetectionsFrame>, BusError> {
        self.publisher(topics::DETECTIONS).await
    }
    pub async fn subscribe_detections(&self) -> Result<TypedSubscriber<DetectionsFrame>, BusError> {
        self.subscriber(topics::DETECTIONS).await
    }
    pub async fn publish_telemetry(&self) -> Result<TypedPublisher<TelemetrySample>, BusError> {
        self.publisher(topics::TELEMETRY).await
    }
    pub async fn subscribe_telemetry(&self) -> Result<TypedSubscriber<TelemetrySample>, BusError> {
        self.subscriber(topics::TELEMETRY).await
    }

    /// Graceful-закрытие сессии.
    pub async fn close(self) -> Result<(), BusError> {
        self.session.close().await.map_err(zerr)
    }
}

fn zerr(e: impl std::fmt::Display) -> BusError {
    BusError::Zenoh(e.to_string())
}

/// Издатель типизированных сообщений (serde_json payload).
pub struct TypedPublisher<T> {
    inner: zenoh::pubsub::Publisher<'static>,
    _marker: std::marker::PhantomData<fn(T)>,
}

impl<T: Serialize> TypedPublisher<T> {
    /// Опубликовать значение (put, best-effort).
    pub async fn publish(&self, value: &T) -> Result<(), BusError> {
        let payload = serde_json::to_vec(value)?;
        self.inner.put(payload).await.map_err(zerr)
    }
}

/// Подписчик типизированных сообщений (FIFO-канал).
pub struct TypedSubscriber<T> {
    inner: zenoh::pubsub::Subscriber<
        zenoh::handlers::FifoChannelHandler<zenoh::sample::Sample>,
    >,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> TypedSubscriber<T> {
    /// Дождаться следующего сообщения.
    pub async fn recv(&self) -> Result<T, BusError> {
        let sample = self
            .inner
            .recv_async()
            .await
            .map_err(|e| BusError::Zenoh(e.to_string()))?;
        Ok(serde_json::from_slice(sample.payload().to_bytes().as_ref())?)
    }

    /// Дождаться следующего сообщения с таймаутом.
    pub async fn recv_timeout(&self, timeout: Duration) -> Result<T, BusError> {
        tokio::time::timeout(timeout, self.recv())
            .await
            .map_err(|_| BusError::Timeout)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-process пара сессий: listener + connector, сквозной обмен
    /// телеметрией и детекциями (критерий «обмен сообщениями»).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn typed_pub_sub_roundtrip() {
        let bus = EventBus::listen(BusConfig {
            endpoint: "tcp/127.0.0.1:17447".into(),
            ..BusConfig::default()
        })
        .await
        .unwrap();
        let client = EventBus::connect("tcp/127.0.0.1:17447").await.unwrap();

        let sub = client.subscribe_telemetry().await.unwrap();
        let pub_ = bus.publish_telemetry().await.unwrap();
        // Declare-ы асинхронно размножаются между peer-ами: даём им
        // распространиться до первой публикации (иначе потеря первых
        // сообщений — ожидаемая семантика zenoh до готовности подписки).
        tokio::time::sleep(Duration::from_millis(300)).await;

        let sample = TelemetrySample {
            t_ms: 1234,
            roll_deg: 1.5,
            pitch_deg: -0.25,
            yaw_deg: 90.0,
            alt_m: 120.5,
        };
        pub_.publish(&sample).await.unwrap();
        let got = sub.recv_timeout(Duration::from_secs(5)).await.unwrap();
        assert_eq!(got, sample);

        // Детекции (common::Detection roundtrip через serde).
        let dsub = bus.subscribe_detections().await.unwrap();
        let dpub = client.publish_detections().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let det = DetectionsFrame {
            frame_seq: 7,
            captured_at: chrono::Utc::now(),
            detections: vec![Detection {
                bbox: common::BoundingBox {
                    x: 10,
                    y: 20,
                    width: 30,
                    height: 40,
                },
                class: "person".into(),
                class_id: 0,
                confidence: 0.87,
                frame_seq: 7,
                detected_at: chrono::Utc::now(),
            }],
        };
        dpub.publish(&det).await.unwrap();
        let got = dsub.recv_timeout(Duration::from_secs(5)).await.unwrap();
        assert_eq!(got.frame_seq, 7);
        assert_eq!(got.detections.len(), 1);
        assert_eq!(got.detections[0].class, "person");
        assert!((got.detections[0].confidence - 0.87).abs() < 1e-6);

        bus.close().await.unwrap();
        client.close().await.unwrap();
    }
}
