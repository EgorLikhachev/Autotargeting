//! # shmem-buffer — SPMC кольцевой буфер кадров в разделяемой памяти
//!
//! Задача **TG26-160**: видеокадры в едином формате доступны нескольким
//! независимым компонентам (детектор, классификатор, трекер, видеозапись)
//! без копий и без передачи пикселей через шину сообщений.
//!
//! ## Модель
//!
//! * **Один продюсер** ([`FrameProducer`]) пишет кадры в кольцевой буфер
//!   фиксированной ёмкости в разделяемой памяти.
//! * **Много потребителей** ([`FrameConsumer`]: `Clone` в процессе, отдельный
//!   `attach` из другого процесса) читают **одни и те же кадры** zero-copy.
//! * Кадр не перезаписывается, пока хоть один потребитель держит его
//!   ([`FrameGuard`] RAII: `Drop` освобождает слот, panic-safe).
//! * Заполненный буфер / медленный потребитель — **drop-new** (Вариант A):
//!   продюсер выбрасывает новый кадр и никогда не блокируется; свежий кадр
//!   доступен сразу после освобождения слота.
//!
//! ## Формат хранения
//!
//! [`StorageFormat::Nv12`] (4:2:0 semi-planar, `w*h*3/2` — совпадает с
//! конвенцией `video-capture::convert`) — дефолт; `Rgb24` — альтернатива.
//! Конвертация источников в NV12 — `video_capture::convert::*_to_nv12`.
//!
//! ## Пример (внутри процесса)
//!
//! ```
//! use shmem_buffer::{create_in_process, RingConfig, StorageFormat, NextStep};
//!
//! let cfg = RingConfig { capacity: 8, width: 64, height: 48,
//!                         format: StorageFormat::Nv12 };
//! let producer = create_in_process(&cfg).unwrap();
//! let consumer = producer.consumer();
//!
//! let frame = vec![0u8; cfg.frame_size() as usize];
//! producer.publish(&frame, shmem_buffer::now_ns()).unwrap();
//!
//! match consumer.next_after(0) {
//!     NextStep::Frame(guard) => {
//!         assert_eq!(guard.frame_id(), 1);
//!         // guard разыменовывается в &[u8] пиксельных данных
//!     }
//!     _ => unreachable!("just published"),
//! }
//! ```
//!
//! ## Пример (между процессами, Linux)
//!
//! ```no_run
//! # fn main() -> Result<(), shmem_buffer::RingError> {
//! // Процесс A (продюсер):
//! let producer = shmem_buffer::create_shared("autotarget.frames",
//!     &shmem_buffer::RingConfig::default())?;
//! // Процесс B..N (потребители, независимые бинарники):
//! let consumer = shmem_buffer::attach_shared("autotarget.frames")?;
//! # Ok(()) }
//! ```
//!
//! ## Синхронизация
//!
//! Без мьютексов: одно атомарное слово на слот (`ref_count`, где
//! `u32::MAX` = writer-lock). Продюсер и читатели соревнуют CAS на одном
//! слове, поэтому «чтение начато» и «запись начата» взаимно исключены по
//! построению; seqlock-валидация `frame_id` страхует от оборота кольца.
//! Детали и доказательство — ADR D-013 и `docs/DEV_NOTES/shmem_ring_buffer.md`.
//!
//! ## Ограничения (зафиксированы в DEV_NOTES)
//!
//! * SPMC: мульти-продюсер не поддерживается (камера одна).
//! * Оповещение потребителей — polling (`next_after`/`latest`).
//! * Крэш потребителя оставляет слот занятым до римера
//!   ([`recover_stale_slots`]); продюсер при этом продолжает работу.
//! * Межпроцессный режим — Linux (memfd + linkat + mmap).

pub mod layout;
pub mod ring;
pub mod shm;

pub use layout::{
    frame_size, segment_size, BufferHeader, SlotMeta, FORMAT_NV12, FORMAT_RGB24,
    LAYOUT_VERSION, MAGIC, WRITER_LOCK,
};
pub use ring::{
    now_ns, recover_stale_slots, ts_ns_to_datetime, DropReason, FrameConsumer, FrameGuard,
    FrameProducer, FrameView, NextStep, ProducerStats, PublishResult, RingConfig, RingError,
    StorageFormat,
};
pub use shm::{attach_shared, create_in_process, create_shared, remove_segment, segment_path};

/// Проверка покрытия критериев готовности TG26-160 (интеграционно —
/// `tests/acceptance.rs`; вызов из кода не требуется).
#[doc(hidden)]
pub const TASK_ID: &str = "TG26-160";
