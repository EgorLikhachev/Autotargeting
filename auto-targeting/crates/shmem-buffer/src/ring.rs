//! SPMC кольцевой буфер поверх региона памяти (TG26-160, ADR D-013).
//!
//! Модель: один продюсер (`FrameProducer`), много независимых потребителей
//! (`FrameConsumer`, клонируется). Работает и межпроцессно (регион —
//! `mmap(MAP_SHARED)`, см. [`crate::shm`]), и внутрипроцессно (арена).
//!
//! # Протокол (одно атомарное слово на слот)
//!
//! `SlotMeta::ref_count`: `0` = кадр готов, читателей нет; `1..=MAX-1` =
//! готов, столько читателей держат; [`WRITER_LOCK`] = продюсер пишет.
//!
//! * Продюсер забирает слот `CAS(0 → WRITER_LOCK)`. Неудача (слот держат
//!   читатели) → **drop-new**: кадр выбрасывается, `write_seq` не двигается,
//!   `dropped_frames++` (Вариант A — решён заказчиком).
//! * Читатель берёт слот CAS-цепочкой на том же слове: `CAS(0 → 1)` либо
//!   `CAS(n → n+1)` при `n > 0`. Успех → продюсер физически не может войти
//!   (его CAS `0 → MAX` требует нуля) — перезапись исключена по построению.
//! * После взятия слота читатель валидирует `frame_id` (seqlock-страховка от
//!   полного оборота кольца за время между выбором кадра и CAS).
//!
//! `FrameGuard` (RAII) декрементирует счётчик в `Drop` — счётчик не зависнет
//! при панике потребителя (в dev-профиле unwind, guard корректно дропается;
//! в release `panic=abort` процесс умирает целиком — см. ример).

use std::alloc::{alloc, dealloc, Layout};
use std::mem::size_of;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use common::{Frame, FrameMetadata, PixelFormat};

use crate::layout::{
    data_area_offset, frame_size as layout_frame_size, segment_size, BufferHeader, SlotMeta,
    FORMAT_NV12, FORMAT_RGB24, LAYOUT_VERSION, MAGIC, WRITER_LOCK,
};

/// Конфигурация хранилища кадров.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageFormat {
    /// 4:2:0 semi-planar, `w*h*3/2` — дефолт (NPU-предпочтительный).
    Nv12,
    /// Пакованный RGB, `w*h*3`.
    Rgb24,
}

impl StorageFormat {
    #[must_use]
    pub fn code(self) -> u32 {
        match self {
            Self::Nv12 => FORMAT_NV12,
            Self::Rgb24 => FORMAT_RGB24,
        }
    }
    #[must_use]
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            FORMAT_NV12 => Some(Self::Nv12),
            FORMAT_RGB24 => Some(Self::Rgb24),
            _ => None,
        }
    }
    #[must_use]
    pub fn pixel_format(self) -> PixelFormat {
        match self {
            Self::Nv12 => PixelFormat::Nv12,
            Self::Rgb24 => PixelFormat::Rgb24,
        }
    }
}

/// Параметры создаваемого кольца.
#[derive(Debug, Clone, Copy)]
pub struct RingConfig {
    pub capacity: u32,
    pub width: u32,
    pub height: u32,
    pub format: StorageFormat,
}

impl Default for RingConfig {
    fn default() -> Self {
        Self {
            capacity: 10,
            width: 640,
            height: 480,
            format: StorageFormat::Nv12,
        }
    }
}

impl RingConfig {
    #[must_use]
    pub fn frame_size(&self) -> u32 {
        layout_frame_size(self.format.code(), self.width, self.height)
            .expect("RingConfig validated at construction")
    }
    pub(crate) fn validate(&self) -> Result<(), RingError> {
        if self.capacity == 0 || self.capacity > 4096 {
            return Err(RingError::InvalidConfig("capacity must be 1..=4096".into()));
        }
        if self.frame_size_check().is_none() {
            return Err(RingError::InvalidConfig(format!(
                "unsupported geometry {}x{} {:?} (NV12 requires even dims)",
                self.width, self.height, self.format
            )));
        }
        let total = segment_size(self.capacity, self.frame_size_check().unwrap_or(0));
        if total > 256 * 1024 * 1024 {
            return Err(RingError::InvalidConfig(format!(
                "segment {total} bytes exceeds 256 MiB cap"
            )));
        }
        Ok(())
    }
    fn frame_size_check(&self) -> Option<u32> {
        layout_frame_size(self.format.code(), self.width, self.height)
    }
}

/// Ошибки кольца/SHM.
#[derive(thiserror::Error, Debug)]
pub enum RingError {
    #[error("invalid frame size: expected {expected}, got {got}")]
    InvalidFrameSize { expected: usize, got: usize },
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("region corrupted: {0}")]
    Corrupted(&'static str),
    #[error("shared memory error: {0}")]
    Shm(String),
    #[error("not supported on this platform")]
    UnsupportedOs,
    #[error("segment already exists: {0}")]
    SegmentExists(String),
    #[error("attach failed: {0}")]
    AttachFailed(String),
}

/// Unix epoch, наносекунды.
#[must_use]
pub fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// ===================== Region =====================

#[allow(dead_code)] // Mapped/External конструируются только в linux-сборке
pub(crate) enum Backing {
    /// Куча (тесты/бенчи, выравнивание 4096 как у mmap).
    Heap(Layout),
    /// mmap-регион; fd закрывается при разрушении.
    Mapped { fd: i32, len: usize, unlink: Option<String> },
    /// Регион нам не принадлежит (не освобождается).
    External,
}

/// Провалидированный вид сегмента. Живёт в `Arc` — `FrameGuard` держит
/// клон, поэтому отображение не освободится под ногами читателя.
pub(crate) struct Region {
    ptr: NonNull<u8>,
    len: usize,
    capacity: u32,
    frame_size: u32,
    backing: Backing,
}

// Region доступен из нескольких потоков/процессов только через атомики
// (ref_count/frame_id) и read-only данные после публикации; &mut нет нигде.
unsafe impl Send for Region {}
unsafe impl Sync for Region {}

impl Region {
    /// Инициализировать новый сегмент в арене (куча, выравнивание 4096).
    /// Для тестов, бенчей и как референс-инициализация перед mmap.
    pub(crate) fn init_heap(cfg: &RingConfig) -> Result<Arc<Self>, RingError> {
        cfg.validate()?;
        let fs = cfg.frame_size();
        let len = segment_size(cfg.capacity, fs);
        let layout = Layout::from_size_align(len, 4096)
            .map_err(|e| RingError::Shm(format!("layout: {e}")))?;
        // SAFETY: layout has non-zero size (validated).
        let ptr = unsafe { alloc(layout) };
        let Some(ptr) = NonNull::new(ptr) else {
            return Err(RingError::Shm("arena alloc failed".into()));
        };
        // КРИТИЧНО: alloc::alloc НЕ обнуляет память. Без явного зануления
        // слоты получают мусорные ref_count/frame_id из переиспользованной
        // кучи — publish падает CAS-ом на «занятом» слоте (проявлялось
        // каскадом order-dependent фейлов после освобождения памяти
        // стресс-тестом). mmap-путь (shm.rs) обнулён ядром (ftruncate).
        // SAFETY: ptr валиден для записи len байт.
        unsafe { std::ptr::write_bytes(ptr.as_ptr(), 0, len) };
        // SAFETY: ptr — свежая обнулённая аллокация, регион ещё не опубликован.
        let region = unsafe { Self::init_raw(ptr, len, cfg, Backing::Heap(layout)) };
        Ok(region)
    }

    /// Записать заголовок в только что выделенный (обнулённый) регион.
    /// `magic` пишется последним с Release — attach валидирует его первым.
    ///
    /// # Safety
    /// `ptr` указывает на `len` обнулённых байт с выравниванием ≥ 8;
    /// регион принадлежит исключительно вызывающему до возврата.
    pub(crate) unsafe fn init_raw(
        ptr: NonNull<u8>,
        len: usize,
        cfg: &RingConfig,
        backing: Backing,
    ) -> Arc<Self> {
        let fs = cfg.frame_size();
        debug_assert_eq!(len, segment_size(cfg.capacity, fs));
        let region = Arc::new(Self {
            ptr,
            len,
            capacity: cfg.capacity,
            frame_size: fs,
            backing,
        });
        region.write_header_init(cfg);
        region
    }

    /// Провалидировать существующий регион (после mmap/attach).
    ///
    /// # Safety
    /// `ptr` указывает как минимум на `len` корректных байт с выравниванием ≥ 8;
    /// содержимое могло быть записано другим процессом.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // используется shm.rs (linux)
    pub(crate) unsafe fn attach_raw(
        ptr: NonNull<u8>,
        len: usize,
        backing: Backing,
    ) -> Result<Arc<Self>, RingError> {
        if len < size_of::<BufferHeader>() {
            return Err(RingError::AttachFailed("segment shorter than header".into()));
        }
        let h = &*ptr.as_ptr().cast::<BufferHeader>();
        if h.magic.load(Ordering::Acquire) != MAGIC {
            return Err(RingError::AttachFailed("bad magic".into()));
        }
        if h.layout_version != LAYOUT_VERSION {
            return Err(RingError::AttachFailed(format!(
                "layout version {} != {LAYOUT_VERSION}",
                h.layout_version
            )));
        }
        let capacity = h.capacity;
        let frame_size = h.frame_size;
        if capacity == 0 || capacity > 4096 {
            return Err(RingError::AttachFailed("bad capacity".into()));
        }
        if len != segment_size(capacity, frame_size) {
            return Err(RingError::AttachFailed(format!(
                "segment len {len} != expected {}",
                segment_size(capacity, frame_size)
            )));
        }
        Ok(Arc::new(Self {
            ptr,
            len,
            capacity,
            frame_size,
            backing,
        }))
    }

    #[must_use]
    pub(crate) fn header(&self) -> &BufferHeader {
        // SAFETY: ptr выровнен (≥4096), len ≥ size_of::<BufferHeader>()
        // проверено при создании; ссылка read-only, &mut не создаётся.
        unsafe { &*self.ptr.as_ptr().cast::<BufferHeader>() }
    }

    /// Сырой указатель на заголовок — для записи plain-полей при инициализации
    /// без материализации `&mut` (конкурентные читатели держат `&`).
    #[must_use]
    pub(crate) fn header_ptr_mut(&self) -> *mut BufferHeader {
        self.ptr.as_ptr().cast()
    }

    /// Сырой указатель на слот — аналогично `header_ptr_mut`.
    #[must_use]
    pub(crate) fn slot_ptr_mut(&self, idx: u32) -> *mut SlotMeta {
        debug_assert!(idx < self.capacity);
        let off = size_of::<BufferHeader>() + idx as usize * size_of::<SlotMeta>();
        // SAFETY: внутри провалидированного региона.
        unsafe { self.ptr.as_ptr().add(off).cast() }
    }

    /// Инициализация заголовка (однократно, до публикации magic).
    ///
    /// # Safety
    /// Регион только что создан и никому больше не виден.
    pub(crate) unsafe fn write_header_init(&self, cfg: &RingConfig) {
        let hp = self.header_ptr_mut();
        std::ptr::addr_of_mut!((*hp).layout_version).write(LAYOUT_VERSION);
        std::ptr::addr_of_mut!((*hp).capacity).write(cfg.capacity);
        std::ptr::addr_of_mut!((*hp).width).write(cfg.width);
        std::ptr::addr_of_mut!((*hp).height).write(cfg.height);
        std::ptr::addr_of_mut!((*hp).format).write(cfg.format.code());
        std::ptr::addr_of_mut!((*hp).frame_size).write(cfg.frame_size());
        std::ptr::addr_of_mut!((*hp).write_seq).write(AtomicU64::new(0));
        std::ptr::addr_of_mut!((*hp).dropped_frames).write(AtomicU64::new(0));
        std::ptr::addr_of_mut!((*hp).created_ns).write(now_ns());
        std::ptr::addr_of_mut!((*hp).producer_pid).write(std::process::id());
        // magic последним: Release-публикация заголовка для attach.
        (*std::ptr::addr_of_mut!((*hp).magic)).store(MAGIC, Ordering::Release);
    }

    /// Запись plain-полей метаданных слота (фаза WRITER_LOCK: читателей нет).
    ///
    /// # Safety
    /// Вызывающий держит WRITER_LOCK на слоте `idx`.
    pub(crate) unsafe fn write_slot_meta(&self, idx: u32, ts_ns: u64) {
        let h = self.header();
        let sp = self.slot_ptr_mut(idx);
        std::ptr::addr_of_mut!((*sp).ts_ns).write(ts_ns);
        std::ptr::addr_of_mut!((*sp).width).write(h.width);
        std::ptr::addr_of_mut!((*sp).height).write(h.height);
        std::ptr::addr_of_mut!((*sp).format).write(h.format);
    }

    #[must_use]
    pub(crate) fn slot(&self, idx: u32) -> &SlotMeta {
        debug_assert!(idx < self.capacity);
        let off = size_of::<BufferHeader>() + idx as usize * size_of::<SlotMeta>();
        // SAFETY: off + size_of::<SlotMeta>() ≤ data_area_offset ≤ len.
        unsafe { &*self.ptr.as_ptr().add(off).cast::<SlotMeta>() }
    }

    #[must_use]
    pub(crate) fn data_ptr(&self, idx: u32) -> NonNull<u8> {
        debug_assert!(idx < self.capacity);
        let off = data_area_offset(self.capacity) + idx as usize * self.frame_size as usize;
        debug_assert!(off + self.frame_size as usize <= self.len);
        // SAFETY: внутри провалидированного региона.
        unsafe { NonNull::new_unchecked(self.ptr.as_ptr().add(off)) }
    }

    #[must_use]
    pub(crate) fn frame_size(&self) -> u32 {
        self.frame_size
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        match self.backing {
            Backing::Heap(layout) => unsafe { dealloc(self.ptr.as_ptr(), layout) },
            Backing::Mapped { fd, len, ref unlink } => {
                teardown_mapped(fd, self.ptr, len, unlink);
            }
            Backing::External => {}
        }
    }
}

/// Освобождение mmap-региона. Mapped-бэкинг создаётся только на Linux
/// (shm.rs); на других хостах вариант не конструируется — no-op.
#[cfg(target_os = "linux")]
fn teardown_mapped(fd: i32, ptr: NonNull<u8>, len: usize, unlink: &Option<String>) {
    unsafe {
        libc::munmap(ptr.as_ptr().cast(), len);
        libc::close(fd);
    }
    if let Some(path) = unlink {
        if let Ok(c) = std::ffi::CString::new(path.as_str()) {
            unsafe { libc::unlink(c.as_ptr()) };
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn teardown_mapped(fd: i32, _ptr: NonNull<u8>, _len: usize, _unlink: &Option<String>) {
    // Недостижимо: Mapped создаётся только в linux-части shm.rs.
    let _ = fd;
}

// ===================== Producer =====================

/// Почему кадр был выброшен (drop-new, Вариант A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// Слот занят читателями (медленный потребитель держит кадр).
    HeldByReaders { slot: u32, readers: u32 },
    /// Слот под WRITER_LOCK (прошлая запись не завершилась — крэш продюсера;
    /// лечение — `recover_stale_slots`).
    WriterLockHeld { slot: u32 },
}

/// Результат публикации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishResult {
    Published { frame_id: u64 },
    Dropped { reason: DropReason },
}

/// Сводка статистики продюсера.
#[derive(Debug, Clone, Copy)]
pub struct ProducerStats {
    pub published: u64,
    pub dropped: u64,
}

/// Единственный писатель кольца.
pub struct FrameProducer {
    region: Arc<Region>,
}

impl FrameProducer {
    pub(crate) fn new(region: Arc<Region>) -> Self {
        Self { region }
    }

    /// Конфигурация кольца (ёмкость/размеры/формат).
    #[must_use]
    pub fn config(&self) -> RingConfig {
        let h = self.region.header();
        RingConfig {
            capacity: h.capacity,
            width: h.width,
            height: h.height,
            format: StorageFormat::from_code(h.format).unwrap_or(StorageFormat::Nv12),
        }
    }

    /// Опубликовать кадр. `data.len()` обязан равняться `frame_size`
    /// конфигурации; `ts_ns` — метка времени (unix epoch, нс).
    ///
    /// Политика заполненного буфера — **drop-new** (Вариант A): если слот
    /// под следующий кадр ещё держит читатель, новый кадр выбрасывается,
    /// продюсер никогда не блокируется.
    pub fn publish(&self, data: &[u8], ts_ns: u64) -> Result<PublishResult, RingError> {
        let h = self.region.header();
        let expected = self.region.frame_size() as usize;
        if data.len() != expected {
            return Err(RingError::InvalidFrameSize {
                expected,
                got: data.len(),
            });
        }

        // Единственный писатель: собственные записи видимы нам без синхронизации.
        let seq = h.write_seq.load(Ordering::Relaxed);
        let next_id = seq
            .checked_add(1)
            .ok_or(RingError::Corrupted("frame id overflow"))?;
        let idx = u32::try_from(next_id % u64::from(self.region_capacity()))
            .expect("capacity <= 4096 fits u32");
        let slot = self.region.slot(idx);
        let _ = idx; // используется в write_slot_meta ниже

        // Забрать слот. Успех возможен только из состояния 0.
        if slot
            .ref_count
            .compare_exchange(0, WRITER_LOCK, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            let cur = slot.ref_count.load(Ordering::Relaxed);
            h.dropped_frames.fetch_add(1, Ordering::Relaxed);
            let reason = if cur == WRITER_LOCK {
                DropReason::WriterLockHeld { slot: idx }
            } else {
                DropReason::HeldByReaders {
                    slot: idx,
                    readers: cur,
                }
            };
            return Ok(PublishResult::Dropped { reason });
        }

        // Размер проверяем ПОСЛЕ захвата слота — вернуть слот при ошибке.
        // (Проверка выше уже отсекла неверный размер, но оставлена защита.)
        debug_assert_eq!(data.len(), expected);

        // Пишем данные и метаданные. Читателей нет (ref был 0, мы держим
        // WRITER_LOCK) — обычные записи безопасны.
        let dst = self.region.data_ptr(idx);
        // SAFETY: dst..dst+frame_size внутри региона; src — проверенный срез.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst.as_ptr(), expected);
        }
        // SAFETY: слот под WRITER_LOCK (мы его взяли CAS выше).
        unsafe { self.region.write_slot_meta(idx, ts_ns) };
        slot.frame_id.store(next_id, Ordering::Release);

        // ОТПУСК СЛОТА: кадр опубликован, слот снова доступен читателям
        // (их CAS 0→1 спаривается с этим Release — данные видны целиком).
        // БЕЗ этой строки слот навсегда оставался в WRITER_LOCK (баг,
        // из-за которого stress-тест зависал бесконечно).
        slot.ref_count.store(0, Ordering::Release);

        // Публикация: write_seq последним (Release) — читатель по Acquire-load
        // write_seq видит данные, метаданные и отпущенный слот.
        h.write_seq.store(next_id, Ordering::Release);

        Ok(PublishResult::Published { frame_id: next_id })
    }

    #[must_use]
    pub fn stats(&self) -> ProducerStats {
        let h = self.region.header();
        ProducerStats {
            published: h.write_seq.load(Ordering::Acquire),
            dropped: h.dropped_frames.load(Ordering::Relaxed),
        }
    }

    /// Удобство: опубликовать с текущей меткой времени.
    pub fn publish_now(&self, data: &[u8]) -> Result<PublishResult, RingError> {
        self.publish(data, now_ns())
    }

    #[must_use]
    pub fn consumer(&self) -> FrameConsumer {
        FrameConsumer::new(Arc::clone(&self.region))
    }

    #[must_use]
    fn region_capacity(&self) -> u32 {
        self.region.header().capacity
    }
}

// ===================== Consumer / Guard =====================

/// Стабильный снимок метаданных кадра (после взятия слота).
#[derive(Debug, Clone, Copy)]
pub struct FrameView {
    pub frame_id: u64,
    pub ts_ns: u64,
    pub width: u32,
    pub height: u32,
    /// `FORMAT_*` из layout.
    pub format: u32,
}

impl FrameView {
    #[must_use]
    pub fn storage_format(&self) -> Option<StorageFormat> {
        StorageFormat::from_code(self.format)
    }
    #[must_use]
    pub fn to_metadata(&self) -> FrameMetadata {
        FrameMetadata {
            width: self.width,
            height: self.height,
            format: self.storage_format().map_or(PixelFormat::Nv12, StorageFormat::pixel_format),
            captured_at: ts_ns_to_datetime(self.ts_ns),
            seq: self.frame_id,
        }
    }
}

#[must_use]
pub fn ts_ns_to_datetime(ts_ns: u64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(
        (ts_ns / 1_000_000_000) as i64,
        (ts_ns % 1_000_000_000) as u32,
    )
    .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
}

/// Результат `next_after`.
#[derive(Debug)]
pub enum NextStep {
    /// Искомый кадр получен.
    Frame(FrameGuard),
    /// `last_id + 1` ещё не опубликован — потребитель догнал продюсера.
    UpToDate,
    /// Потребитель отстал больше чем на ёмкость кольца: искомый кадр уже
    /// перезаписан. Нужно прыгнуть на `latest` (или перезапустить стрим).
    TooFarBehind { last_seen: u64, latest: u64 },
}

/// RAII-держатель кадра. `Deref` → пиксельные данные; `Drop` освобождает
/// слот (декремент ref_count). Живёт — слот не перезаписывается.
pub struct FrameGuard {
    region: Arc<Region>,
    slot_idx: u32,
    view: FrameView,
    data: NonNull<u8>,
    len: usize,
}

// Guard владеет долей региона (Arc) и read-only срез данных; перемещение
// между потоками безопасно.
unsafe impl Send for FrameGuard {}
unsafe impl Sync for FrameGuard {}

impl std::fmt::Debug for FrameGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameGuard")
            .field("view", &self.view)
            .field("len", &self.len)
            .finish()
    }
}

impl std::ops::Deref for FrameGuard {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: data..data+len внутри живого региона (Arc держит его),
        // продюсер исключён протоколом, данные иммутабельны для читателя.
        unsafe { std::slice::from_raw_parts(self.data.as_ptr(), self.len) }
    }
}

impl Drop for FrameGuard {
    fn drop(&mut self) {
        // Release: пара к Acquire при взятии — данные дочитаны до декремента.
        self.region.slot(self.slot_idx).ref_count.fetch_sub(1, Ordering::Release);
    }
}

impl FrameGuard {
    #[must_use]
    pub fn view(&self) -> FrameView {
        self.view
    }
    #[must_use]
    pub fn frame_id(&self) -> u64 {
        self.view.frame_id
    }
    #[must_use]
    pub fn ts_ns(&self) -> u64 {
        self.view.ts_ns
    }
    /// Копия в owning-тип проекта (для legacy-потребителей поверх каналов).
    #[must_use]
    pub fn to_frame(&self) -> Frame {
        Frame {
            data: self.to_vec(),
            metadata: self.view.to_metadata(),
        }
    }
}

/// Независимый потребитель. `Clone` — несколько потребителей в одном
/// процессе; в других процессах — свой `attach`.
#[derive(Clone)]
pub struct FrameConsumer {
    region: Arc<Region>,
}

/// Число спин-попыток взятия слота перед возвратом «занято».
const ACQUIRE_SPINS: usize = 64;

enum Acquired {
    Got(FrameGuard),
    /// Слот под записью / CAS-гонка — повторить вызов позже.
    Busy,
    /// Под слотом уже другой frame_id (кольцо обернулось) — `id` актуальный.
    Mismatch { actual: u64 },
}

impl FrameConsumer {
    pub(crate) fn new(region: Arc<Region>) -> Self {
        Self { region }
    }

    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.region.header().capacity
    }

    /// Идентификатор последнего опубликованного кадра (0 — ничего нет).
    #[must_use]
    pub fn latest_id(&self) -> u64 {
        self.region.header().write_seq.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn dropped_frames(&self) -> u64 {
        self.region.header().dropped_frames.load(Ordering::Relaxed)
    }

    /// Свежий кадр. `None` — кадров нет либо слот мгновенно занят записью
    /// (вызовите снова; это нормальная polling-семантика).
    #[must_use]
    pub fn latest(&self) -> Option<FrameGuard> {
        let seq = self.latest_id();
        if seq == 0 {
            return None;
        }
        let id = seq;
        match self.try_acquire(id % u64::from(self.capacity()), None) {
            Acquired::Got(g) => Some(g),
            _ => None,
        }
    }

    /// Следующий кадр после `last_id` (сквозная последовательность —
    /// режим трекера). `last_id = 0` — с самого первого доступного.
    pub fn next_after(&self, last_id: u64) -> NextStep {
        // Микро-оптимизация: capacity поднимаем из header ОДИН раз (раньше
        // до трёх загрузок за итерацию), u64-деление — только на входе.
        let cap = u64::from(self.capacity());
        // Ограниченный retry: каждый виток перечитывает актуальный write_seq,
        // поэтому зацикливание невозможно (want монотонно догоняет seq).
        for _ in 0..ACQUIRE_SPINS * 4 {
            let seq = self.latest_id();
            if seq == 0 {
                return NextStep::UpToDate;
            }
            let want = last_id.saturating_add(1);
            if want > seq {
                return NextStep::UpToDate;
            }
            if seq - want >= cap {
                return NextStep::TooFarBehind {
                    last_seen: last_id,
                    latest: seq,
                };
            }
            match self.try_acquire(want % cap, Some(want)) {
                Acquired::Got(g) => return NextStep::Frame(g),
                // Между load write_seq и CAS продюсер ушёл вперёд — новый
                // виток с свежим seq (Mismatch→UpToDate, Busy→повтор).
                Acquired::Mismatch { actual } => {
                    // Кольцо ушло вперёд за время CAS — следующий виток
                    // увидит актуальный seq (UpToDate/TooFarBehind).
                    tracing::trace!(want, actual, "slot advanced");
                    continue;
                }
                Acquired::Busy => continue,
            }
        }
        NextStep::UpToDate
    }

    /// Базовый примитив взятия слота.
    ///
    /// `expected_id` — seqlock-валидация после CAS (защита от полного
    /// оборота кольца между выбором кадра и взятием слота).
    fn try_acquire(&self, slot_idx: u64, expected_id: Option<u64>) -> Acquired {
        let idx = u32::try_from(slot_idx % u64::from(self.capacity()))
            .expect("capacity <= 4096");
        let slot = self.region.slot(idx);
        for _ in 0..ACQUIRE_SPINS {
            let cur = slot.ref_count.load(Ordering::Relaxed);
            let target = match cur {
                WRITER_LOCK => {
                    std::hint::spin_loop();
                    continue;
                }
                0 => 1,
                n => n + 1,
            };
            if slot
                .ref_count
                .compare_exchange(cur, target, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                // Слот наш: продюсер исключён (его CAS требует 0 при target==1,
                // а при n>0 запись запрещена протоколом).
                slot.holder_pid.store(std::process::id(), Ordering::Relaxed);
                let actual = slot.frame_id.load(Ordering::Relaxed);
                if let Some(want) = expected_id {
                    if actual != want {
                        slot.ref_count.fetch_sub(1, Ordering::Release);
                        return Acquired::Mismatch { actual };
                    }
                }
                let view = FrameView {
                    frame_id: actual,
                    ts_ns: slot.ts_ns,
                    width: slot.width,
                    height: slot.height,
                    format: slot.format,
                };
                let len = self.region.frame_size() as usize;
                return Acquired::Got(FrameGuard {
                    region: Arc::clone(&self.region),
                    slot_idx: idx,
                    view,
                    data: self.region.data_ptr(idx),
                    len,
                });
            }
            std::hint::spin_loop();
        }
        Acquired::Busy
    }
}

// ===================== Recovery =====================

/// Проверка живости процесса (`kill(pid, 0)`): `true` — существует.
#[cfg(target_os = "linux")]
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    // Вне Linux (тесты на x86-хостах) считаем все pid живыми — ример
    // работает только в реальном окружении.
    pid != 0
}

/// Ример: освободить слоты, зависшие из-за умерших процессов.
///
/// * `ref_count == WRITER_LOCK` при мёртвом `producer_pid` → слот отпущен.
/// * `ref_count > 0` при мёртвом `holder_pid` И метке кадра старше
///   `max_age_ns` → слот отпущен (двойная проверка снижает гонку с
///   читателем, который только что взял слот, но ещё не записал pid).
///
/// Возвращает число освобождённых слотов. Инструмент оператора, не автозов.
pub fn recover_stale_slots(consumer: &FrameConsumer, max_age_ns: u64) -> usize {
    let region = &consumer.region;
    let h = region.header();
    let producer_dead = h.producer_pid != 0 && !pid_alive(h.producer_pid);
    let now = now_ns();
    let mut freed = 0usize;
    for idx in 0..h.capacity {
        let slot = region.slot(idx);
        let cur = slot.ref_count.load(Ordering::Relaxed);
        let cleared = if cur == WRITER_LOCK {
            producer_dead
                && slot
                    .ref_count
                    .compare_exchange(WRITER_LOCK, 0, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
        } else if cur > 0 {
            let pid = slot.holder_pid.load(Ordering::Relaxed);
            let stale = now.saturating_sub(slot.ts_ns) > max_age_ns;
            let dead = pid != 0 && pid != std::process::id() && !pid_alive(pid);
            // Двойная проверка: pid мог обновиться живым читателем.
            let pid_now = slot.holder_pid.load(Ordering::Relaxed);
            dead && stale && pid_now == pid
                && slot
                    .ref_count
                    .compare_exchange(cur, 0, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
        } else {
            false
        };
        if cleared {
            freed += 1;
        }
    }
    freed
}

#[cfg(test)]
impl FrameConsumer {
    /// Тестовый хелпер: кадр по конкретному id (внутри окна).
    fn latest_by_id(&self, id: u64) -> Option<FrameGuard> {
        match self.try_acquire(id % u64::from(self.capacity()), Some(id)) {
            Acquired::Got(g) => Some(g),
            _ => None,
        }
    }
}

// ===================== Tests =====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::FIRST_FRAME_ID;

    fn cfg(cap: u32) -> RingConfig {
        RingConfig {
            capacity: cap,
            width: 64,
            height: 48,
            format: StorageFormat::Nv12,
        }
    }

    fn frame_bytes(cfg: &RingConfig, fill: u8) -> Vec<u8> {
        vec![fill; cfg.frame_size() as usize]
    }

    /// Паттерн для детекции torn-read: каждое u32-слово кадра = frame_id.
    fn pattern_frame(cfg: &RingConfig, id: u64) -> Vec<u8> {
        let mut v = Vec::with_capacity(cfg.frame_size() as usize);
        for _ in 0..cfg.frame_size() / 4 {
            v.extend_from_slice(&(id as u32).to_le_bytes());
        }
        v.resize(cfg.frame_size() as usize, 0);
        v
    }

    #[test]
    fn publish_then_latest_roundtrip() {
        let c = cfg(4);
        let prod = FrameProducer::new(Region::init_heap(&c).unwrap());
        let cons = prod.consumer();

        assert_eq!(cons.latest_id(), 0);
        assert!(cons.latest().is_none());

        let data = pattern_frame(&c, FIRST_FRAME_ID);
        match prod.publish(&data, 123_456).unwrap() {
            PublishResult::Published { frame_id } => assert_eq!(frame_id, 1),
            other => panic!("expected Published, got {other:?}"),
        }
        let g = cons.latest().expect("frame");
        assert_eq!(g.frame_id(), 1);
        assert_eq!(g.ts_ns(), 123_456);
        assert_eq!(g.view().width, 64);
        assert_eq!(g.view().storage_format(), Some(StorageFormat::Nv12));
        // Целостность данных.
        assert_eq!(&g[..8], &1u32.to_le_bytes()[..].repeat(2));
        drop(g);
        assert_eq!(prod.stats().published, 1);
    }

    #[test]
    fn two_consumers_hold_same_frame() {
        let c = cfg(4);
        let prod = FrameProducer::new(Region::init_heap(&c).unwrap());
        let a = prod.consumer();
        let b = a.clone();

        prod.publish(&pattern_frame(&c, 1), 1).unwrap();
        let ga = a.latest().unwrap();
        let gb = b.latest().unwrap();
        assert_eq!(ga.frame_id(), gb.frame_id());
        assert_eq!(ga.to_vec(), gb.to_vec());
        // Пока жив хоть один guard — ref_count > 0.
        drop(ga);
        drop(gb);
    }

    /// Ключевой сценарий: слот держат читатели → publish возвращает Dropped,
    /// данные в слоте НЕ меняются (перепроверка после освобождения).
    #[test]
    fn no_overwrite_while_held_drop_new() {
        let c = cfg(3);
        let prod = FrameProducer::new(Region::init_heap(&c).unwrap());
        let cons = prod.consumer();

        // Заполняем кольцо (id 1..=3).
        for id in 1..=3 {
            prod.publish(&pattern_frame(&c, id), id).unwrap();
        }
        // Держим слот id=1 (slot 1 % 3 = 1).
        let held = cons.latest_by_id(1).expect("frame 1");

        // Новый кадр id=4 должен попасть в слот 4%3=1 — он занят.
        match prod.publish(&pattern_frame(&c, 4), 4).unwrap() {
            PublishResult::Dropped {
                reason: DropReason::HeldByReaders { slot, readers },
            } => {
                assert_eq!(slot, 1);
                assert!(readers >= 1);
            }
            other => panic!("expected Dropped, got {other:?}"),
        }
        // Данные держимого кадра не тронуты.
        assert_eq!(&held[..4], &1u32.to_le_bytes());
        assert_eq!(prod.stats().dropped, 1);
        assert_eq!(prod.stats().published, 3);

        drop(held);
        // Слот освободился — публикация проходит.
        match prod.publish(&pattern_frame(&c, 4), 4).unwrap() {
            PublishResult::Published { frame_id } => assert_eq!(frame_id, 4),
            other => panic!("expected Published after release, got {other:?}"),
        }
    }

    #[test]
    fn next_after_sequence_walk_and_wrap() {
        // Окно = capacity: серия 1..=6 при capacity=6 проходится целиком.
        let c = cfg(6);
        let prod = FrameProducer::new(Region::init_heap(&c).unwrap());
        let cons = prod.consumer();

        for id in 1..=6 {
            prod.publish(&pattern_frame(&c, id), id).unwrap();
        }
        // Полный проход 1..=6 без потерь.
        let mut last = 0u64;
        loop {
            match cons.next_after(last) {
                NextStep::Frame(g) => {
                    assert_eq!(g.frame_id(), last + 1);
                    last = g.frame_id();
                }
                NextStep::UpToDate => break,
                NextStep::TooFarBehind { .. } => panic!("not expected here"),
            }
        }
        assert_eq!(last, 6);

        // Публикуем ещё 4 (итого 10 > capacity+last_seen=6) → walk с 0
        // теперь корректно отклоняется: окно хранит только 5..=10.
        for id in 7..=10 {
            prod.publish(&pattern_frame(&c, id), id).unwrap();
        }
        match cons.next_after(0) {
            NextStep::TooFarBehind { latest, .. } => assert_eq!(latest, 10),
            other => panic!("expected TooFarBehind, got {other:?}"),
        }
        // А свежая часть окна по-прежнему проходится.
        match cons.next_after(7) {
            NextStep::Frame(g) => assert_eq!(g.frame_id(), 8),
            other => panic!("expected Frame(8), got {other:?}"),
        }
    }

    #[test]
    fn wrong_size_rejected_and_slot_not_leaked() {
        let c = cfg(4);
        let prod = FrameProducer::new(Region::init_heap(&c).unwrap());
        let short = vec![0u8; 10];
        match prod.publish(&short, 1) {
            Err(RingError::InvalidFrameSize { expected, got }) => {
                assert_eq!(expected, c.frame_size() as usize);
                assert_eq!(got, 10);
            }
            other => panic!("expected InvalidFrameSize, got {other:?}"),
        }
        // Кольцо не сдвинулось — следующий publish идёт в слот id=1.
        let cons = prod.consumer();
        prod.publish(&frame_bytes(&c, 0xAA), 1).unwrap();
        assert_eq!(cons.latest().unwrap().frame_id(), 1);
    }

    /// Паника потребителя внутри scope с guard → счётчик корректно
    /// возвращается (RAII, unwind в dev-профиле).
    #[test]
    fn guard_survives_panic() {
        let c = cfg(4);
        let prod = FrameProducer::new(Region::init_heap(&c).unwrap());
        let cons = prod.consumer();
        prod.publish(&frame_bytes(&c, 1), 1).unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = cons.latest().unwrap();
            panic!("consumer died mid-read");
        }));
        assert!(result.is_err());

        // Слот должен быть свободен: publish в тот же слот (id 1 % 4 = 1 →
        // следующий id=5 займёт слот 1) проходит.
        for id in 2..=5u64 {
            prod.publish(&frame_bytes(&c, id as u8), id).unwrap();
        }
        let stats = prod.stats();
        assert_eq!(stats.published, 5);
        assert_eq!(stats.dropped, 0, "no drops expected after panic-unwind");
    }

    /// Стресс: 1 продюсер + 3 читателя, torn-read детектор (все u32-слова
    /// кадра равны frame_id). Гонки протокола проявились бы как несведение
    /// слов или несоответствие id.
    #[test]
    fn stress_three_readers_no_torn_reads() {
        let c = cfg(8);
        let prod = std::sync::Arc::new(FrameProducer::new(Region::init_heap(&c).unwrap()));
        let frames_total = 600u64;

        let mut handles = Vec::new();
        for _ in 0..3 {
            let p = Arc::clone(&prod);
            handles.push(std::thread::spawn(move || {
                let cons = p.consumer();
                let mut verified = 0u64;
                let mut last = 0u64;
                // Жёсткий дедлайн: баг протокола должен валить тест паникой,
                // а не подвешивать набор навсегда.
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_secs(60);
                // Выход не по магическому числу кадров (drop-new может
                // выбросить часть — финальный id < frames_total), а по
                // «стрим стих»: серия UpToDate без роста latest_id.
                let mut quiet_polls: u32 = 0;
                let mut seen_latest = 0u64;
                loop {
                    if std::time::Instant::now() > deadline {
                        panic!("stress timeout: reader stuck at frame {last}");
                    }
                    match cons.next_after(last) {
                        NextStep::Frame(g) => {
                            let id = g.frame_id();
                            // Torn-read детектор: каждое слово = id.
                            for w in g.chunks_exact(4) {
                                let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
                                assert_eq!(v, id as u32, "torn read in frame {id}");
                            }
                            last = id;
                            verified += 1;
                            quiet_polls = 0;
                        }
                        NextStep::UpToDate => {
                            let now_latest = cons.latest_id();
                            if now_latest == seen_latest {
                                quiet_polls += 1;
                            } else {
                                quiet_polls = 0;
                                seen_latest = now_latest;
                            }
                            // ~200 пустых опросов без роста стрима = конец.
                            if quiet_polls >= 200 {
                                break;
                            }
                            // Отдаём квант продюсеру: жёсткий спин морил его
                            // голодом на мало-ядерных хостах.
                            std::thread::yield_now();
                        }
                        NextStep::TooFarBehind { latest, .. } => {
                            last = latest; // прыгаем на свежий — валидно
                        }
                    }
                }
                // Читатель сошёлся к концу стрима.
                assert_eq!(last, cons.latest_id(), "reader did not converge");
                verified
            }));
        }
        for id in 1..=frames_total {
            let _ = prod.publish(&pattern_frame(&c, id), id).unwrap();
        }
        let verified: u64 = handles.into_iter().map(|h| h.join().expect("reader")).sum();
        // Цель стресса — детектор torn-read (assert внутри читателей):
        // ни один прочитанный кадр не был перезаписан под ногами. Читатели
        // могут прыгать (TooFarBehind), покрытие 100% не гарантируется.
        assert!(verified > 0, "readers verified nothing");
        let s = prod.stats();
        // drop-new: при удержании слотов читателями часть кадров выброшена.
        assert!(s.published <= frames_total);
        assert!(s.published > 0);
    }

    /// Конфигурации с ошибками отклоняются на создании.
    #[test]
    fn invalid_configs_rejected() {
        let mut c = cfg(4);
        c.capacity = 0;
        assert!(matches!(
            Region::init_heap(&c).err(),
            Some(RingError::InvalidConfig(_))
        ));
        let mut odd = cfg(4);
        odd.width = 63; // NV12 требует чётных
        assert!(matches!(
            Region::init_heap(&odd).err(),
            Some(RingError::InvalidConfig(_))
        ));
    }

    /// to_frame() даёт owning-копию с корректной метадатой.
    #[test]
    fn to_frame_conversion() {
        let c = cfg(4);
        let prod = FrameProducer::new(Region::init_heap(&c).unwrap());
        prod.publish(&frame_bytes(&c, 7), 1_700_000_000_000_000_000).unwrap();
        let g = prod.consumer().latest().unwrap();
        let f = g.to_frame();
        assert_eq!(f.metadata.width, 64);
        assert_eq!(f.metadata.height, 48);
        assert_eq!(f.metadata.format, PixelFormat::Nv12);
        assert_eq!(f.metadata.seq, 1);
        assert_eq!(f.data.len(), c.frame_size() as usize);
    }
}
