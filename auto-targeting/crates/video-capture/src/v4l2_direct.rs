//! Direct V4L2 capture via raw ioctl — bypasses the `v4l` crate abstraction
//! that was 5× slower than `v4l2-ctl` on Orange Pi 5 (21 FPS vs 100 FPS).
//!
//! Uses `libc` for ioctl + mmap, no external C dependencies beyond
//! the kernel V4L2 interface. Linux-only.
//!
//! ## Performance
//! On Arducam OV9782 (USB 2.0, MJPG):
//!   - `v4l` crate (MMapStream): ~21 FPS sustained
//!   - This direct ioctl path: ~90-100 FPS (matches v4l2-ctl)

#![cfg(target_os = "linux")]

use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use common::{Frame, FrameMetadata, PixelFormat};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::traits::{VideoCaptureError, VideoResult, VideoSource};

// === V4L2 ioctl number computation (matches kernel _IO macros on 64-bit Linux) ===
// _IOC(dir, type, nr, size) = (dir << 30) | (size << 16) | (type << 8) | nr
// dir: NONE=0, WRITE=1, READ=2, READWRITE=3
const fn _ioc(dir: u32, typ: u32, nr: u32, size: u32) -> u64 {
    ((dir as u64) << 30) | ((size as u64) << 16) | ((typ as u64) << 8) | nr as u64
}

// Struct sizes on 64-bit Linux aarch64 (verified via gcc on Orange Pi 5).
const SZ_CAP: u32 = 104; // sizeof(v4l2_capability)
const SZ_FMT: u32 = 208; // sizeof(v4l2_format)
const SZ_PARM: u32 = 204; // sizeof(v4l2_streamparm)
const SZ_REQBUFS: u32 = 20; // sizeof(v4l2_requestbuffers)
const SZ_BUF: u32 = 88; // sizeof(v4l2_buffer)
const SZ_INT: u32 = 4; // sizeof(int)

const TYP_V: u32 = b'V' as u32;

const VIDIOC_S_FMT: u64 = _ioc(3, TYP_V, 5, SZ_FMT);
const VIDIOC_S_PARM: u64 = _ioc(3, TYP_V, 22, SZ_PARM);
const VIDIOC_REQBUFS: u64 = _ioc(3, TYP_V, 8, SZ_REQBUFS);
const VIDIOC_QUERYBUF: u64 = _ioc(3, TYP_V, 9, SZ_BUF);
const VIDIOC_QBUF: u64 = _ioc(3, TYP_V, 15, SZ_BUF);
const VIDIOC_DQBUF: u64 = _ioc(3, TYP_V, 17, SZ_BUF);
const VIDIOC_STREAMON: u64 = _ioc(1, TYP_V, 18, SZ_INT);
const VIDIOC_STREAMOFF: u64 = _ioc(1, TYP_V, 19, SZ_INT);

const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
const V4L2_MEMORY_MMAP: u32 = 1;

const V4L2_PIX_FMT_MJPEG: u32 = 0x47504a4d;
const V4L2_PIX_FMT_YUYV: u32 = 0x56595559;
const V4L2_PIX_FMT_NV12: u32 = 0x3231564e;

// === V4L2 structs (sizes MUST match kernel on 64-bit Linux) ===
// We use repr(C) + explicit padding to match sizeof() exactly.

/// v4l2_pix_format (48 bytes) — the C struct pix format.
#[repr(C)]
struct V4l2PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}
// 12 × 4 = 48 bytes ✓

/// v4l2_format (208 bytes). The kernel's `union fmt` has 8-byte alignment
/// (because v4l2_window contains a pointer), so there's 4 bytes of PADDING
/// between `type` (u32) and the union. Total = 4 + 4(pad) + 200(union) = 208.
#[repr(C)]
struct V4l2Format {
    typ: u32,                  // offset 0-3
    _pad0: u32,                // offset 4-7 (kernel padding for 8-byte aligned union)
    pix: V4l2PixFormat,        // offset 8-55 (first 48 bytes of the 200-byte union)
    _pad: [u8; 152],           // offset 56-207 (rest of union: 200-48=152)
}

/// v4l2_requestbuffers (20 bytes).
#[repr(C)]
struct V4l2RequestBuffers {
    count: u32,
    typ: u32,
    memory: u32,
    capabilities: u32,
    flags: u32,
}
// 5 × 4 = 20 ✓

/// v4l2_buffer (88 bytes). Field layout matches kernel exactly on 64-bit.
#[repr(C)]
struct V4l2Buffer {
    index: u32,
    typ: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    _pad0: u32, // alignment padding before timeval (8-byte aligned)
    ts_sec: i64,
    ts_usec: i64,
    timecode: [u8; 24],
    sequence: u32,
    memory: u32,
    // union m (offset/userptr/planes/fd) — 8 bytes on 64-bit
    m_offset: u32,
    _pad1: u32,
    length: u32,
    reserved2: u32,
}
// 5×4 + pad(4) + 2×8 + 24 + 2×4 + 4+pad(4) + 2×4
// = 20 + 4 + 16 + 24 + 8 + 8 + 8 = 88 ✓

impl Default for V4l2Buffer {
    fn default() -> Self {
        Self {
            index: 0,
            typ: V4L2_BUF_TYPE_VIDEO_CAPTURE,
            bytesused: 0,
            flags: 0,
            field: 0,
            _pad0: 0,
            ts_sec: 0,
            ts_usec: 0,
            timecode: [0u8; 24],
            sequence: 0,
            memory: V4L2_MEMORY_MMAP,
            m_offset: 0,
            _pad1: 0,
            length: 0,
            reserved2: 0,
        }
    }
}

/// v4l2_streamparm — we only need timeperframe, so use raw bytes.
type V4l2StreamParm = [u8; 204];

/// Mapped buffer.
struct MappedBuffer {
    ptr: *mut std::ffi::c_void,
    length: usize,
}

impl Drop for MappedBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                let _ = libc::munmap(self.ptr, self.length);
            }
        }
    }
}

unsafe impl Send for MappedBuffer {}
unsafe impl Sync for MappedBuffer {}

/// Direct V4L2 video source via raw ioctl.
pub struct V4l2DirectSource {
    device: String,
    width: u32,
    height: u32,
    fps: u32,
    format: PixelFormat,
    num_buffers: u32,
    stop_flag: Option<Arc<AtomicBool>>,
}

impl V4l2DirectSource {
    pub fn new(device: impl Into<String>, width: u32, height: u32, fps: u32) -> Self {
        Self {
            device: device.into(),
            width,
            height,
            fps,
            format: PixelFormat::Mjpeg,
            num_buffers: 4,
            stop_flag: None,
        }
    }

    pub fn with_format(mut self, format: PixelFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_buffers(mut self, n: u32) -> Self {
        self.num_buffers = n.clamp(2, 8);
        self
    }

    fn pixel_format_to_fourcc(fmt: PixelFormat) -> u32 {
        match fmt {
            PixelFormat::Mjpeg => V4L2_PIX_FMT_MJPEG,
            PixelFormat::Yuyv => V4L2_PIX_FMT_YUYV,
            PixelFormat::Nv12 => V4L2_PIX_FMT_NV12,
            PixelFormat::Rgb24 => V4L2_PIX_FMT_YUYV,
        }
    }
}

#[async_trait]
impl VideoSource for V4l2DirectSource {
    async fn start(&mut self) -> VideoResult<mpsc::Receiver<Frame>> {
        let (tx, rx) = mpsc::channel(self.num_buffers as usize);

        let stop_flag = Arc::new(AtomicBool::new(false));
        self.stop_flag = Some(Arc::clone(&stop_flag));

        let device_path = self.device.clone();
        let width = self.width;
        let height = self.height;
        let fps = self.fps;
        let format = self.format;
        let num_buffers = self.num_buffers;
        let fourcc = Self::pixel_format_to_fourcc(format);

        info!(
            device = %device_path,
            width, height, fps, num_buffers,
            "starting V4L2 direct-ioctl capture"
        );

        std::thread::spawn(move || {
            let result = run_direct_capture(
                &device_path, width, height, fps, fourcc, format, num_buffers, tx, stop_flag,
            );
            if let Err(e) = result {
                error!(error = %e, "V4L2 direct capture thread exited with error");
            } else {
                info!("V4L2 direct capture thread exited cleanly");
            }
        });

        Ok(rx)
    }

    async fn stop(&mut self) -> VideoResult<()> {
        if let Some(flag) = &self.stop_flag {
            flag.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "V4l2DirectSource"
    }
}

fn run_direct_capture(
    device_path: &str,
    width: u32,
    height: u32,
    fps: u32,
    fourcc: u32,
    format: PixelFormat,
    num_buffers: u32,
    tx: mpsc::Sender<Frame>,
    stop_flag: Arc<AtomicBool>,
) -> VideoResult<()> {
    // 1. Open device.
    let fd = unsafe {
        let c_path = std::ffi::CString::new(device_path).map_err(|e| {
            VideoCaptureError::DeviceOpen(format!("invalid path: {e}"))
        })?;
        let ret = libc::open(c_path.as_ptr(), libc::O_RDWR);
        if ret < 0 {
            return Err(VideoCaptureError::DeviceOpen(format!(
                "open {device_path}: {}",
                std::io::Error::last_os_error()
            )));
        }
        ret
    };

    // 2. Set format (VIDIOC_S_FMT).
    let mut fmt = V4l2Format {
        typ: V4L2_BUF_TYPE_VIDEO_CAPTURE,
        _pad0: 0,
        pix: V4l2PixFormat {
            width,
            height,
            pixelformat: fourcc,
            field: 0,
            bytesperline: 0,
            sizeimage: 0,
            colorspace: 0,
            priv_: 0,
            flags: 0,
            ycbcr_enc: 0,
            quantization: 0,
            xfer_func: 0,
        },
        _pad: [0u8; 152],
    };
    unsafe { ioctl(fd, VIDIOC_S_FMT, &mut fmt)? };
    let neg_w = fmt.pix.width;
    let neg_h = fmt.pix.height;
    // Buffer size from S_FMT — some drivers don't fill buf.length in QUERYBUF,
    // so we use sizeimage as the mmap length.
    let buf_size = if fmt.pix.sizeimage > 0 {
        fmt.pix.sizeimage as usize
    } else {
        (neg_w as usize * neg_h as usize * 3).max(1024) // fallback estimate
    };
    eprintln!("[v4l2-direct] S_FMT: {}x{} pixfmt=0x{:x} buf_size={}",
        neg_w, neg_h, fmt.pix.pixelformat, buf_size);
    info!(width = neg_w, height = neg_h, "V4L2 direct format negotiated");

    // 3. Set frame rate (VIDIOC_S_PARM).
    if fps > 0 {
        let mut parm: V4l2StreamParm = [0u8; 204];
        // Write type (offset 0, u32) + timeperframe (numerator at offset 8, denominator at offset 12).
        parm[0..4].copy_from_slice(&V4L2_BUF_TYPE_VIDEO_CAPTURE.to_ne_bytes());
        parm[8..12].copy_from_slice(&1u32.to_ne_bytes()); // numerator
        parm[12..16].copy_from_slice(&fps.to_ne_bytes()); // denominator
        unsafe { ioctl(fd, VIDIOC_S_PARM, parm.as_mut_ptr() as *mut _)? };
    }

    // 4. Request MMAP buffers.
    let mut req = V4l2RequestBuffers {
        count: num_buffers,
        typ: V4L2_BUF_TYPE_VIDEO_CAPTURE,
        memory: V4L2_MEMORY_MMAP,
        capabilities: 0,
        flags: 0,
    };
    unsafe { ioctl(fd, VIDIOC_REQBUFS, &mut req)? };
    let n_bufs = req.count;
    debug!(buffers = n_bufs, "V4L2 direct buffers allocated");

    // 5. Query + mmap each buffer, then queue.
    let mut mapped: Vec<MappedBuffer> = Vec::with_capacity(n_bufs as usize);
    for i in 0..n_bufs {
        let mut buf = V4l2Buffer::default();
        buf.index = i;
        unsafe { ioctl(fd, VIDIOC_QUERYBUF, &mut buf)? };
        // DIAGNOSTIC: dump bytes around union m and length to compare with C kernel struct.
        let raw_ptr = &buf as *const V4l2Buffer as *const u8;
        eprintln!("[v4l2-direct] QUERYBUF buf[{}] raw bytes 64-88:", i);
        for off in (64..88).step_by(4) {
            let val = unsafe { *(raw_ptr.add(off) as *const u32) };
            eprintln!("  offset {}: 0x{:08x} ({})", off, val, val);
        }
        let len = buf.length as usize;
        let offset = buf.m_offset as usize;
        eprintln!("[v4l2-direct] m_offset={} length={}", offset, len);
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                offset as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            let err = std::io::Error::last_os_error();
            return Err(VideoCaptureError::DeviceConfig(format!(
                "mmap buffer {i} failed: len={len} offset={offset} fd={fd} err={err}"
            )));
        }
        mapped.push(MappedBuffer { ptr, length: len });

        // Queue this buffer.
        buf.typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        unsafe { ioctl(fd, VIDIOC_QBUF, &mut buf)? };
    }

    // 6. Start streaming.
    let mut stream_on = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    unsafe { ioctl(fd, VIDIOC_STREAMON, &mut stream_on)? };
    info!("V4L2 direct streaming started");

    // 7. Capture loop.
    let mut seq: u64 = 0;
    loop {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }

        // Dequeue (VIDIOC_DQBUF) — hot path.
        let mut buf = V4l2Buffer::default();
        buf.typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        match unsafe { ioctl(fd, VIDIOC_DQBUF, &mut buf) } {
            Ok(()) => {}
            Err(e) => {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                // EAGAIN is normal in non-blocking mode; we use blocking so it's unexpected.
                warn!(error = %e, "V4L2 direct dequeue error");
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
        }

        // Copy frame data from mmap.
        let data_len = buf.bytesused as usize;
        let buf_idx = buf.index as usize;
        let frame_data = if buf_idx < mapped.len() && data_len > 0 {
            unsafe {
                std::slice::from_raw_parts(mapped[buf_idx].ptr as *const u8, data_len).to_vec()
            }
        } else {
            Vec::new()
        };

        // Re-queue the buffer immediately (before processing).
        buf.typ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        unsafe {
            let _ = ioctl(fd, VIDIOC_QBUF, &mut buf);
        }

        // Build + send Frame (drop-old via try_send).
        let frame = Frame {
            data: frame_data,
            metadata: FrameMetadata {
                width: neg_w,
                height: neg_h,
                format,
                captured_at: Utc::now(),
                seq,
            },
        };

        match tx.try_send(frame) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => break,
            Err(mpsc::error::TrySendError::Full(_)) => {
                debug!("V4L2 direct: dropping frame {seq}");
            }
        }

        seq += 1;
    }

    // 8. Stop + cleanup.
    let mut stream_off = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    unsafe {
        let _ = ioctl(fd, VIDIOC_STREAMOFF, &mut stream_off);
    }
    drop(mapped);
    unsafe {
        libc::close(fd);
    }
    info!(frames_captured = seq, "V4L2 direct capture thread done");
    Ok(())
}

/// Raw ioctl wrapper.
unsafe fn ioctl<T>(fd: libc::c_int, request: u64, arg: *mut T) -> VideoResult<()> {
    let ret = libc::ioctl(fd, request as _, arg as *mut _);
    if ret < 0 {
        let err = std::io::Error::last_os_error();
        return Err(VideoCaptureError::Capture(format!(
            "ioctl(0x{request:x}) failed: {err}"
        )));
    }
    Ok(())
}
