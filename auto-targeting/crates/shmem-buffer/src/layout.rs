//! Layout контрактов разделяемой памяти (TG26-160).
//!
//! Все структуры — `#[repr(C)]` с фиксированными смещениями: по ним сможет
//! работать C++-потребитель (будущий FFI), и они защищены unit-тестами
//! размеров/офсетов (урок `v4l2_buffer`: неявный padding недопустим).
//!
//! Сегмент:
//! ```text
//! offset 0                      BufferHeader (64 B, cache-line aligned)
//! + 64                          SlotMeta[capacity] (каждый 64 B)
//! + 64 * capacity               data[capacity][frame_size]
//! ```
//!
//! Формат кадра: NV12 (4:2:0 semi-planar, `w*h*3/2`: Y-плоскость @0,
//! interleaved UV @`w*h`) — конвенция совпадает с
//! `video-capture/src/convert.rs`. Rgb24 — альтернативный режим.

use std::sync::atomic::{AtomicU32, AtomicU64};

/// Magic сегмента: little-endian `b"ATFB"` (Auto-Targeting Frames Buffer).
pub const MAGIC: u32 = u32::from_le_bytes(*b"ATFB");

/// Версия лэйаута. Несовпадение у attach-клиента → ошибка.
pub const LAYOUT_VERSION: u16 = 1;

/// Код формата NV12 в поле `format` (C-совместимый дискриминант).
pub const FORMAT_NV12: u32 = 0;
/// Код формата RGB24 (packed, `w*h*3`).
pub const FORMAT_RGB24: u32 = 1;

/// Сентинел «продюсер пишет в слот» в `SlotMeta::ref_count`.
pub const WRITER_LOCK: u32 = u32::MAX;

/// Идентификаторы кадров начинаются с 1; `0` в `SlotMeta::frame_id`
/// означает «слот ни разу не публиковался».
pub const FIRST_FRAME_ID: u64 = 1;

/// Заголовок сегмента. Пишется один раз при создании (кроме атомиков),
/// читается всеми процессами.
///
/// Инвариант публикации: слот кадра с идентификатором `F` равен
/// `F % capacity`; `write_seq` = количество опубликованных кадров =
/// идентификатор последнего.
#[repr(C, align(64))]
pub struct BufferHeader {
    /// Всегда [`MAGIC`]; пишется ПОСЛЕДНИМ при инициализации (Release),
    /// attach валидирует первым (Acquire).
    pub magic: AtomicU32,
    pub layout_version: u16,
    pub capacity: u32,
    pub width: u32,
    pub height: u32,
    /// `FORMAT_NV12` | `FORMAT_RGB24`.
    pub format: u32,
    /// Размер данных кадра в байтах (одинаков для всех слотов).
    pub frame_size: u32,
    pub write_seq: AtomicU64,
    /// Счётчик дропов по drop-new политике (слот занят читателями).
    pub dropped_frames: AtomicU64,
    /// Unix epoch, наносекунды.
    pub created_ns: u64,
    /// pid создателя (для римера зависших WRITER_LOCK).
    pub producer_pid: u32,
}

/// Метаданные слота. `ref_count` — единственное слово синхронизации слота:
///
/// * `0` — кадр готов, читателей нет; продюсер может забрать слот
///   (`CAS 0 → WRITER_LOCK`);
/// * `1..MAX-1` — кадр готов, столько читателей держат слот (каждый взял
///   через `CAS 0 → 1` / `fetch_add`); перезапись запрещена;
/// * `u32::MAX` ([`WRITER_LOCK`]) — продюсер пишет; читатели ждут/пропускают.
///
/// Обе стороны соревнуются одним CAS на одном слове, поэтому состояния
/// «читатель начал чтение» и «продюсер начал запись» взаимно исключены
/// по построению (см. ADR D-013).
#[repr(C, align(64))]
pub struct SlotMeta {
    pub ref_count: AtomicU32,
    /// Атомарный: читатель пикует его ДО взятия слота (seqlock-валидация).
    pub frame_id: AtomicU64,
    /// Unix epoch, наносекунды. Читается только после CAS — гонок нет.
    pub ts_ns: u64,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    /// pid последнего победителя CAS на слоте (диагностика/ример).
    /// Пишется best-effort, только для восстановления после крэшей.
    pub holder_pid: AtomicU32,
}

/// Размер данных кадра для формата/разрешения, либо `None` (нечётные
/// размеры для NV12, нулевые, переполнение).
#[must_use]
pub fn frame_size(format: u32, width: u32, height: u32) -> Option<u32> {
    if width == 0 || height == 0 {
        return None;
    }
    let px = u64::from(width) * u64::from(height);
    let total = match format {
        // 4:2:0 semi-planar: Y (w*h) + interleaved UV (w*h/2) = w*h*3/2.
        // UV-плоскость требует чётных измерений.
        FORMAT_NV12 => {
            if width % 2 != 0 || height % 2 != 0 {
                return None;
            }
            (px.checked_mul(3)?) >> 1
        }
        FORMAT_RGB24 => px.checked_mul(3)?,
        _ => return None,
    };
    u32::try_from(total).ok()
}

/// Смещение области данных (после заголовка и массива слотов).
#[must_use]
pub fn data_area_offset(capacity: u32) -> usize {
    size_of::<BufferHeader>() + capacity as usize * size_of::<SlotMeta>()
}

/// Полный размер сегмента.
#[must_use]
pub fn segment_size(capacity: u32, frame_size: u32) -> usize {
    data_area_offset(capacity) + capacity as usize * frame_size as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn header_is_exactly_one_cache_line() {
        assert_eq!(size_of::<BufferHeader>(), 64);
    }

    #[test]
    fn slot_meta_is_exactly_one_cache_line() {
        assert_eq!(size_of::<SlotMeta>(), 64);
    }

    /// Фиксация смещений полей — защита от «сдвига» при правках
    /// (урок v4l2_buffer: kernel timeval сместил все поля).
    #[test]
    fn field_offsets_are_pinned() {
        let hdr: BufferHeader = unsafe { std::mem::zeroed() };
        let base = std::ptr::addr_of!(hdr) as usize;
        let off = |p: usize| p - base;
        assert_eq!(off(std::ptr::addr_of!(hdr.magic) as usize), 0);
        assert_eq!(off(std::ptr::addr_of!(hdr.layout_version) as usize), 4);
        assert_eq!(off(std::ptr::addr_of!(hdr.capacity) as usize), 8);
        assert_eq!(off(std::ptr::addr_of!(hdr.width) as usize), 12);
        assert_eq!(off(std::ptr::addr_of!(hdr.height) as usize), 16);
        assert_eq!(off(std::ptr::addr_of!(hdr.format) as usize), 20);
        assert_eq!(off(std::ptr::addr_of!(hdr.frame_size) as usize), 24);
        assert_eq!(off(std::ptr::addr_of!(hdr.write_seq) as usize), 32);
        assert_eq!(off(std::ptr::addr_of!(hdr.dropped_frames) as usize), 40);
        assert_eq!(off(std::ptr::addr_of!(hdr.created_ns) as usize), 48);
        assert_eq!(off(std::ptr::addr_of!(hdr.producer_pid) as usize), 56);

        let slot: SlotMeta = unsafe { std::mem::zeroed() };
        let base = std::ptr::addr_of!(slot) as usize;
        let off = |p: usize| p - base;
        assert_eq!(off(std::ptr::addr_of!(slot.ref_count) as usize), 0);
        assert_eq!(off(std::ptr::addr_of!(slot.frame_id) as usize), 8);
        assert_eq!(off(std::ptr::addr_of!(slot.ts_ns) as usize), 16);
        assert_eq!(off(std::ptr::addr_of!(slot.width) as usize), 24);
        assert_eq!(off(std::ptr::addr_of!(slot.height) as usize), 28);
        assert_eq!(off(std::ptr::addr_of!(slot.format) as usize), 32);
        assert_eq!(off(std::ptr::addr_of!(slot.holder_pid) as usize), 36);
    }

    #[test]
    fn frame_size_math() {
        assert_eq!(frame_size(FORMAT_NV12, 640, 480), Some(460_800));
        assert_eq!(frame_size(FORMAT_RGB24, 640, 480), Some(921_600));
        assert_eq!(frame_size(FORMAT_NV12, 320, 240), Some(115_200));
        assert_eq!(frame_size(FORMAT_NV12, 1280, 720), Some(1_382_400));
        // NV12 требует чётных измерений (UV-плоскость).
        assert_eq!(frame_size(FORMAT_NV12, 321, 240), None);
        assert_eq!(frame_size(FORMAT_NV12, 320, 241), None);
        assert_eq!(frame_size(FORMAT_NV12, 0, 480), None);
        assert_eq!(frame_size(99, 640, 480), None);
    }

    #[test]
    fn segment_layout_math() {
        // 10 слотов NV12 640x480: 64 + 10*64 + 10*460800 = 4_608_704 байта.
        let fs = frame_size(FORMAT_NV12, 640, 480).unwrap();
        assert_eq!(segment_size(10, fs), 64 + 10 * 64 + 10 * 460_800);
        assert_eq!(data_area_offset(10), 64 + 640);
    }
}
