//! Разделяемая память: POSIX shm + mmap (Linux) и заглушка для других хостов.
//!
//! Именование сегментов: объект в `/dev/shm/<name>` (POSIX shared memory
//! семантика — прямой `open`, без libc-символа shm_open). Изначально
//! планировался `memfd_create` + `linkat(AT_EMPTY_PATH)`, но на целевом
//! ядре (6.1.99-rockchip) это эмпирически НЕ работает: AT_EMPTY_PATH
//! возвращает ENOENT, а хардлинк через /proc/self/fd — EXDEV (memfd живёт
//! на другой tmpfs-инстанции). `open("/dev/shm/...")` решает задачу
//! без этих ограничений; mmap-часть и Region не меняются.
//!
//! Деструктор `FrameProducer` удаляет объект (unlink); сами отображения
//! живут, пока живы `Arc<Region>` (включая активные `FrameGuard`).

use crate::ring::{FrameConsumer, FrameProducer, RingConfig, RingError};

/// Полный путь линка сегмента в tmpfs.
#[must_use]
pub fn segment_path(name: &str) -> String {
    format!("/dev/shm/{name}")
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::ptr::NonNull;

    use crate::layout::segment_size;
    use crate::ring::Backing;

    /// Создать сегмент и инициализировать кольцо. Если линк уже существует —
    /// `RingError::SegmentExists` (см. [`remove_segment`]).
    pub fn create_shared(name: &str, cfg: &RingConfig) -> Result<FrameProducer, RingError> {
        cfg.validate()?;
        let c_name = std::ffi::CString::new(name)
            .map_err(|_| RingError::InvalidConfig("name contains NUL".into()))?;
        let path = segment_path(name);
        let c_path = std::ffi::CString::new(path.as_str())
            .map_err(|_| RingError::InvalidConfig("path contains NUL".into()))?;

        // POSIX shm: O_CREAT|O_EXCL в /dev/shm. EEXIST → сегмент уже есть
        // (stale после крэша продюсера) — caller решает (remove_segment).
        // (memfd+linkat(AT_EMPTY_PATH) не работает на целевом ядре:
        // ENOENT/EXDEV — см. док-коммент модуля.)
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EEXIST) {
                return Err(RingError::SegmentExists(name.to_string()));
            }
            return Err(RingError::Shm(format!("open({path}): {e}")));
        }
        let _ = c_name;

        let len = segment_size(cfg.capacity, cfg.frame_size());
        if unsafe { libc::ftruncate(fd, len as libc::off_t) } != 0 {
            let e = std::io::Error::last_os_error();
            unsafe {
                libc::close(fd);
                // Не оставляем мусорный объект.
                libc::unlink(c_path.as_ptr());
            }
            return Err(RingError::Shm(format!("ftruncate: {e}")));
        }

        // mmap обнулённого файла: ftruncate гарантировал нули.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            let e = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(RingError::Shm(format!("mmap: {e}")));
        }
        let ptr =
            NonNull::new(ptr.cast::<u8>()).ok_or_else(|| RingError::Shm("mmap NULL".into()))?;

        // SAFETY: ptr — выровненное (page) mmap-отображение обнулённого
        // сегмента длиной len, принадлежащее только нам.
        let region = unsafe {
            crate::ring::Region::init_raw(
                ptr,
                len,
                cfg,
                Backing::Mapped {
                    fd,
                    len,
                    unlink: Some(path),
                },
            )
        };
        Ok(FrameProducer::new(region))
    }

    /// Подключиться к существующему сегменту как потребитель.
    pub fn attach_shared(name: &str) -> Result<FrameConsumer, RingError> {
        let path = segment_path(name);
        let c_path = std::ffi::CString::new(path.as_str())
            .map_err(|_| RingError::InvalidConfig("path contains NUL".into()))?;
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(RingError::AttachFailed(format!(
                "open {path}: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            let e = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(RingError::AttachFailed(format!("fstat: {e}")));
        }
        let len = st.st_size as usize;
        if len == 0 {
            unsafe { libc::close(fd) };
            return Err(RingError::AttachFailed("segment has zero length".into()));
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            let e = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(RingError::AttachFailed(format!("mmap: {e}")));
        }
        let ptr = NonNull::new(ptr.cast::<u8>())
            .ok_or_else(|| RingError::AttachFailed("mmap NULL".into()))?;
        // SAFETY: ptr — page-aligned mmap длиной len; содержимое валидируется
        // внутри (magic/version/size).
        let region = unsafe {
            crate::ring::Region::attach_raw(
                ptr,
                len,
                Backing::Mapped {
                    fd,
                    len,
                    unlink: None,
                },
            )
        }?;
        Ok(FrameConsumer::new(region))
    }

    /// Удалить именованный линк сегмента (например, застрявший после крэша
    /// продюсера). Активные отображения продолжают работать.
    pub fn remove_segment(name: &str) -> bool {
        let path = segment_path(name);
        match std::ffi::CString::new(path.as_str()) {
            Ok(c) => unsafe { libc::unlink(c.as_ptr()) == 0 },
            Err(_) => false,
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::{attach_shared, create_shared, remove_segment};

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;
    pub fn create_shared(_name: &str, _cfg: &RingConfig) -> Result<FrameProducer, RingError> {
        Err(RingError::UnsupportedOs)
    }
    pub fn attach_shared(_name: &str) -> Result<FrameConsumer, RingError> {
        Err(RingError::UnsupportedOs)
    }
    pub fn remove_segment(_name: &str) -> bool {
        false
    }
}

#[cfg(not(target_os = "linux"))]
pub use imp::{attach_shared, create_shared, remove_segment};

/// Арена в куче — для тестов/бенчей на любом хосте (протокол идентичен SHM).
pub fn create_in_process(cfg: &RingConfig) -> Result<FrameProducer, RingError> {
    Ok(FrameProducer::new(crate::ring::Region::init_heap(cfg)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::{PublishResult, StorageFormat};

    fn cfg() -> RingConfig {
        RingConfig {
            capacity: 4,
            width: 64,
            height: 48,
            format: StorageFormat::Nv12,
        }
    }

    #[test]
    fn in_process_roundtrip_cross_platform() {
        let prod = create_in_process(&cfg()).unwrap();
        let cons = prod.consumer();
        let data = vec![0x5Au8; cfg().frame_size() as usize];
        assert!(matches!(
            prod.publish(&data, 42).unwrap(),
            PublishResult::Published { frame_id: 1 }
        ));
        let g = cons.latest().unwrap();
        assert_eq!(g.ts_ns(), 42);
        assert!(g.iter().all(|&b| b == 0x5A));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shared_segment_create_attach_drop() {
        let name = format!("at-test-{}.frames", std::process::id());
        let _ = remove_segment(&name);

        let prod = create_shared(&name, &cfg()).expect("create");
        // Повторное создание под тем же именем → SegmentExists.
        assert!(matches!(
            create_shared(&name, &cfg()),
            Err(RingError::SegmentExists(_))
        ));

        let data = vec![0xC3u8; cfg().frame_size() as usize];
        assert!(matches!(
            prod.publish(&data, 7).unwrap(),
            PublishResult::Published { .. }
        ));

        let cons = attach_shared(&name).expect("attach");
        let g = cons.latest().expect("frame");
        assert_eq!(g.frame_id(), 1);
        assert!(g.iter().all(|&b| b == 0xC3));
        drop(g);

        drop(prod); // unlink
        assert!(!std::path::Path::new(&segment_path(&name)).exists());
        // Активное отображение консьюмера продолжает работать.
        let stats_id = cons.latest_id();
        assert_eq!(stats_id, 1);
        let _ = remove_segment(&name);
    }
}
