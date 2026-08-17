//! Разделяемая память: memfd + mmap (Linux) и заглушка для других хостов.
//!
//! Именование сегментов: `memfd_create` создаёт анонимный inode; для
//! подключения потребителей из других процессов он линкуется в tmpfs через
//! `linkat(fd, "", AT_FDCWD, "/dev/shm/<name>", AT_EMPTY_PATH)` — для
//! memfd это разрешено без `CAP_DAC_READ_SEARCH` (man memfd_create(2)).
//! attach = `open("/dev/shm/<name>")` + `mmap(MAP_SHARED)`.
//!
//! Деструктор `FrameProducer` удаляет линк; сами отображения живут, пока
//! живы `Arc<Region>` (включая активные `FrameGuard`).

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
    use std::sync::Arc;

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

        // memfd + печать (allow-sealing не нужна, но флаг безвреден и точнее
        // выражает «изменяемый файл в памяти»).
        let fd = unsafe { libc::memfd_create(c_name.as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(RingError::Shm(format!(
                "memfd_create: {}",
                std::io::Error::last_os_error()
            )));
        }

        let len = segment_size(cfg.capacity, cfg.frame_size());
        if unsafe { libc::ftruncate(fd, len as libc::off_t) } != 0 {
            let e = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(RingError::Shm(format!("ftruncate: {e}")));
        }

        // Оубликовать под именем. EEXIST → сегмент уже есть.
        if unsafe {
            libc::linkat(
                fd,
                b"\0".as_ptr().cast(),
                libc::AT_FDCWD,
                c_path.as_ptr(),
                libc::AT_EMPTY_PATH,
            )
        } != 0
        {
            let e = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            if e.raw_os_error() == Some(libc::EEXIST) {
                return Err(RingError::SegmentExists(name.to_string()));
            }
            return Err(RingError::Shm(format!("linkat: {e}")));
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
        let ptr = NonNull::new(ptr.cast::<u8>()).ok_or_else(|| RingError::Shm("mmap NULL".into()))?;

        // SAFETY: ptr — выровненное (page) mmap-отображение обнулённого
        // сегмента длиной len, принадлежащее только нам.
        let region = unsafe {
            crate::ring::Region::init_raw(
                ptr,
                len,
                cfg,
                Backing::Mapped { fd, len, unlink: Some(path) },
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
                Backing::Mapped { fd, len, unlink: None },
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
