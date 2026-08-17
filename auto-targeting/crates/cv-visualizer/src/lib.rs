//! Headless annotation + frame saver for Phase 1.1.
//!
//! Draws bounding boxes, class labels and confidence onto frames and writes:
//!
//! - **Annotated JPEGs** in a frames directory (one per saved frame), suitable
//!   for later ffmpeg muxing into a processed video.
//! - A **JSONL detection log** (`detections.jsonl`) — one line per frame with
//!   `frame_seq`, `timestamp`, and a list of detections. Cheap, structured,
//!   greppable, and the same data the tracker/CLI already uses.
//!
//! ## Why headless / pure-Rust
//!
//! The on-board compute module (RK3588) is headless — there is no display, so
//! a GUI window is pointless and would drag OpenCV/SDL into the build. The
//! task 1.1 criteria explicitly allow "отображаются **или сохраняются**". We
//! render with `image` + `imageproc` (pure Rust, cross-compiles to aarch64),
//! keeping the stack consistent with the rest of the project (which avoids
//! OpenCV in Rust).
//!
//! ## Text rendering
//!
//! Class/confidence labels need a TTF font. To avoid shipping a font binary
//! inside this crate, [`Visualizer`] takes an *optional* loaded font. If none
//! is provided, only the bounding-box rectangles are drawn (class/confidence
//! are still fully recorded in the JSONL log). On most Linux targets a system
//! DejaVu/FreeFont TTF is available — load it via [`Visualizer::with_font_path`].
//!
//! ## Pipeline
//!
//! `Frame` (RGB24) → `RgbImage` → draw hollow bbox rects (+ optional text) →
//! encode JPEG → `FrameWriter::save()` writes JPEG and appends to JSONL.
//!
//! For MJPEG / NV12 frames, decode/convert upstream via
//! `video-capture::convert` first (same as the inference path).

#![deny(unsafe_code)]

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use chrono::Utc;
use common::{BoundingBox, Detection, Frame, PixelFormat};
use image::{ImageBuffer, Rgb, RgbImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_hollow_rect_mut, draw_text_mut};
use imageproc::rect::Rect;
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

// ---- Error type -----------------------------------------------------------

#[derive(Debug, Error)]
pub enum VisualizerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("image encode error: {0}")]
    ImageEncode(String),

    #[error("unsupported pixel format: {0:?} (expected RGB24; convert upstream)")]
    UnsupportedFormat(PixelFormat),

    #[error("frame size mismatch: declared {decl_w}x{decl_h}, data len {data_len}")]
    FrameSize {
        decl_w: u32,
        decl_h: u32,
        data_len: usize,
    },

    #[error("json serialize error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("font load error: {0}")]
    FontLoad(String),
}

pub type VisualizerResult<T> = std::result::Result<T, VisualizerError>;

// ---- Annotation primitives ------------------------------------------------

/// Outline color of the bounding-box rectangle (cyan, high contrast on most
/// scenes). Picked deliberately different from the label background.
pub const BOX_COLOR: Rgb<u8> = Rgb([0, 255, 255]);
/// Background rectangle behind the text label, for legibility.
pub const LABEL_BG: Rgb<u8> = Rgb([0, 0, 0]);
/// Text color (white on the black label background).
pub const TEXT_COLOR: Rgb<u8> = Rgb([255, 255, 255]);

/// Box outline thickness in pixels. We approximate a thick outline by drawing
/// `THICKNESS` hollow rects at increasing insets.
pub const THICKNESS: i32 = 2;

/// Render scale for label text (px tall). Small enough to fit beside boxes on
/// 720p, large enough to read on a 1:1 crop.
pub const LABEL_SCALE: f32 = 14.0;

/// Convert a `Frame` (RGB24) into an `RgbImage` for drawing.
///
/// Only RGB24 is accepted here; MJPEG/YUYV/NV12 must be decoded upstream
/// (same as the inference path).
pub fn frame_to_rgb_image(frame: &Frame) -> VisualizerResult<RgbImage> {
    if frame.metadata.format != PixelFormat::Rgb24 {
        return Err(VisualizerError::UnsupportedFormat(frame.metadata.format));
    }
    let w = frame.metadata.width;
    let h = frame.metadata.height;
    let expected = (w as usize) * (h as usize) * 3;
    if frame.data.len() != expected {
        return Err(VisualizerError::FrameSize {
            decl_w: w,
            decl_h: h,
            data_len: frame.data.len(),
        });
    }
    // `from_raw` only returns None when the buffer length doesn't match
    // w*h*pixel_size; we just checked that, so this is infallible in practice.
    ImageBuffer::from_raw(w, h, frame.data.clone()).ok_or(VisualizerError::FrameSize {
        decl_w: w,
        decl_h: h,
        data_len: expected,
    })
}

/// Draw a hollow rectangle outline of the given thickness by stacking several
/// 1px hollow rects at increasing insets.
fn draw_thick_rect(img: &mut RgbImage, rect: Rect, color: Rgb<u8>, thickness: i32) {
    for i in 0..thickness {
        // Inset by i on each side. `Rect::at` is top-left of the rect; we shift
        // right/down by i and shrink width/height by 2i.
        let x = rect.left() + i;
        let y = rect.top() + i;
        let w = rect.width() as i32 - 2 * i;
        let h = rect.height() as i32 - 2 * i;
        if w > 0 && h > 0 {
            let r = Rect::at(x, y).of_size(w as u32, h as u32);
            draw_hollow_rect_mut(img, r, color);
        }
    }
}

/// Draw a single detection's box + (optional) label onto the image.
///
/// The label is placed just above the box; if it would run off the top edge,
/// it is placed below the box instead.
fn draw_detection(img: &mut RgbImage, det: &Detection, font: Option<&FontVec>) {
    let BoundingBox {
        x,
        y,
        width,
        height,
    } = det.bbox;
    let rect = Rect::at(x as i32, y as i32).of_size(width, height);
    draw_thick_rect(img, rect, BOX_COLOR, THICKNESS);

    let Some(font) = font else {
        return;
    };

    let label = format!("{} {:.2}", det.class, det.confidence);
    let scale = PxScale::from(LABEL_SCALE);
    let scaled = font.as_scaled(scale);

    // Approximate label box size from glyph advances.
    let mut label_w = 0.0f32;
    for c in label.chars() {
        label_w += scaled.h_advance(scaled.glyph_id(c));
    }
    let label_w = label_w.ceil() as u32 + 6;
    let label_h = scaled.height().ceil() as u32 + 4;

    let img_w = img.width();
    let img_h = img.height();

    // Position: prefer above the box; clamp horizontally into image bounds.
    let lx = x.min(img_w.saturating_sub(label_w));
    let ly_above = y.saturating_sub(label_h);
    let ly = if ly_above > 0 {
        ly_above
    } else {
        y + height + 2
    };

    let bg = Rect::at(lx as i32, ly as i32).of_size(label_w.min(img_w), label_h.min(img_h));
    draw_filled_rect_mut(img, bg, LABEL_BG);

    draw_text_mut(
        img,
        TEXT_COLOR,
        lx as i32 + 3,
        ly as i32 + 2,
        scale,
        font,
        &label,
    );
}

/// Annotate a frame with all detections and return the resulting RGB image.
pub fn annotate(
    frame: &Frame,
    detections: &[Detection],
    font: Option<&FontVec>,
) -> VisualizerResult<RgbImage> {
    let mut img = frame_to_rgb_image(frame)?;
    for d in detections {
        draw_detection(&mut img, d, font);
    }
    Ok(img)
}

/// Encode an `RgbImage` to JPEG bytes (quality ~92 — good for debug overlays).
///
/// Принимает кадр **по значению** (перф-аудит 2026-08: прежний `&RgbImage`
/// делал `img.clone()` — лишнюю полнокадровую копию на каждый сохранённый
/// кадр — только чтобы отдать владение DynamicImage).
pub fn encode_jpeg(img: RgbImage) -> VisualizerResult<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::with_capacity(
        (img.width() as usize) * (img.height() as usize) / 4,
    ));
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .map_err(|e| VisualizerError::ImageEncode(e.to_string()))?;
    Ok(buf.into_inner())
}

// ---- Visualizer / FrameWriter --------------------------------------------

/// One JSONL record per saved frame.
#[derive(Debug, Serialize)]
struct DetectionRecord<'a> {
    frame_seq: u64,
    timestamp: String,
    width: u32,
    height: u32,
    n: usize,
    detections: Vec<DetectionJson<'a>>,
}

#[derive(Debug, Serialize)]
struct DetectionJson<'a> {
    class: &'a str,
    class_id: u32,
    confidence: f32,
    bbox: [u32; 4], // [x, y, w, h]
}

/// Writes annotated JPEGs and a JSONL detection log.
///
/// Create once per run with [`FrameWriter::new`], then call [`FrameWriter::save`]
/// for each frame. The `save_every_n` throttle bounds disk usage on long runs
/// while still producing enough frames for a representative processed video.
pub struct FrameWriter {
    frames_dir: PathBuf,
    jsonl_path: PathBuf,
    save_every_n: u64,
    font: Option<FontVec>,
    frame_counter: u64,
    saved_counter: u64,
}

impl FrameWriter {
    /// Create a writer rooted at `output_dir`. Layout:
    ///
    /// ```text
    /// output_dir/
    ///   frames/
    ///     seq_000001.jpg
    ///     seq_000002.jpg
    ///   detections.jsonl
    /// ```
    ///
    /// `save_every_n = 1` saves every frame; `5` saves every 5th (useful for
    /// long soak runs to bound disk usage while still producing a video).
    /// No text labels will be drawn unless [`with_font_path`] is called.
    pub fn new(output_dir: impl AsRef<Path>, save_every_n: u64) -> VisualizerResult<Self> {
        let out = output_dir.as_ref();
        let frames_dir = out.join("frames");
        fs::create_dir_all(&frames_dir)?;
        let jsonl_path = out.join("detections.jsonl");
        Ok(Self {
            frames_dir,
            jsonl_path,
            save_every_n: save_every_n.max(1),
            font: None,
            frame_counter: 0,
            saved_counter: 0,
        })
    }

    /// Load a TTF/OTF font from disk so class/confidence labels are rendered.
    /// Without this call, only bounding-box rectangles are drawn.
    ///
    /// Common system font paths on Linux:
    /// - `/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf`
    /// - `/usr/share/fonts/truetype/freefont/FreeMono.ttf`
    pub fn with_font_path(mut self, path: impl AsRef<Path>) -> VisualizerResult<Self> {
        let bytes = fs::read(path.as_ref())?;
        let font =
            FontVec::try_from_vec(bytes).map_err(|e| VisualizerError::FontLoad(e.to_string()))?;
        self.font = Some(font);
        Ok(self)
    }

    /// True if the next call to [`save`] would actually persist this frame
    /// (i.e. it falls on the every-Nth cadence).
    pub fn should_save(&self) -> bool {
        self.frame_counter % self.save_every_n == 0
    }

    /// Number of frames actually written so far.
    pub fn saved_count(&self) -> u64 {
        self.saved_counter
    }

    /// Whether a font is loaded (i.e. text labels will be rendered).
    pub fn has_font(&self) -> bool {
        self.font.is_some()
    }

    /// Annotate `frame` with `detections`, encode JPEG, write it to
    /// `frames/seq_NNNNNN.jpg`, and append a JSONL record.
    ///
    /// Honours the `save_every_n` throttle — returns `Ok(None)` (and does no
    /// I/O) when the frame is skipped. Always increments the internal counter
    /// so cadence is stable across skipped frames.
    pub fn save(
        &mut self,
        frame: &Frame,
        detections: &[Detection],
    ) -> VisualizerResult<Option<PathBuf>> {
        self.frame_counter += 1;
        if !self.should_save() {
            return Ok(None);
        }

        let annotated = annotate(frame, detections, self.font.as_ref())?;
        let jpeg = encode_jpeg(annotated)?;

        let saved_seq = self.saved_counter + 1;
        let fname = format!("seq_{saved_seq:06}.jpg");
        let path = self.frames_dir.join(&fname);
        fs::write(&path, &jpeg)?;
        self.saved_counter += 1;

        // Append JSONL record.
        let record = DetectionRecord {
            frame_seq: frame.metadata.seq,
            timestamp: Utc::now().to_rfc3339(),
            width: frame.metadata.width,
            height: frame.metadata.height,
            n: detections.len(),
            detections: detections
                .iter()
                .map(|d| DetectionJson {
                    class: d.class.as_str(),
                    class_id: d.class_id,
                    confidence: d.confidence,
                    bbox: [d.bbox.x, d.bbox.y, d.bbox.width, d.bbox.height],
                })
                .collect(),
        };
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.jsonl_path)?;
        let line = serde_json::to_string(&record)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;

        tracing::debug!(
            frame = saved_seq,
            path = %path.display(),
            n = detections.len(),
            "saved annotated frame"
        );
        Ok(Some(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use common::BoundingBox;

    fn rgb_frame(w: u32, h: u32, fill: u8) -> Frame {
        Frame {
            data: vec![fill; (w as usize) * (h as usize) * 3],
            metadata: common::FrameMetadata {
                width: w,
                height: h,
                format: PixelFormat::Rgb24,
                captured_at: Utc::now(),
                seq: 1,
            },
        }
    }

    fn det(x: u32, y: u32, w: u32, h: u32, conf: f32, class: &str) -> Detection {
        Detection {
            bbox: BoundingBox {
                x,
                y,
                width: w,
                height: h,
            },
            class: class.to_string(),
            class_id: 0,
            confidence: conf,
            frame_seq: 1,
            detected_at: Utc::now(),
        }
    }

    #[test]
    fn frame_to_rgb_image_roundtrip() {
        let f = rgb_frame(4, 2, 200);
        let img = frame_to_rgb_image(&f).unwrap();
        assert_eq!(img.dimensions(), (4, 2));
        assert_eq!(img.get_pixel(0, 0), &Rgb([200, 200, 200]));
    }

    #[test]
    fn frame_to_rgb_rejects_nv12() {
        let f = Frame {
            data: vec![0; 12],
            metadata: common::FrameMetadata {
                width: 2,
                height: 2,
                format: PixelFormat::Nv12,
                captured_at: Utc::now(),
                seq: 1,
            },
        };
        assert!(matches!(
            frame_to_rgb_image(&f),
            Err(VisualizerError::UnsupportedFormat(PixelFormat::Nv12))
        ));
    }

    #[test]
    fn frame_to_rgb_rejects_size_mismatch() {
        let f = Frame {
            data: vec![0; 10], // wrong size for 4x2 RGB24 (=24 bytes)
            metadata: common::FrameMetadata {
                width: 4,
                height: 2,
                format: PixelFormat::Rgb24,
                captured_at: Utc::now(),
                seq: 1,
            },
        };
        assert!(matches!(
            frame_to_rgb_image(&f),
            Err(VisualizerError::FrameSize { .. })
        ));
    }

    #[test]
    fn annotate_does_not_panic_on_empty_detections() {
        let f = rgb_frame(64, 48, 30);
        let img = annotate(&f, &[], None).unwrap();
        assert_eq!(img.dimensions(), (64, 48));
    }

    #[test]
    fn annotate_draws_box_within_bounds() {
        let f = rgb_frame(64, 48, 30);
        let d = det(10, 10, 20, 15, 0.9, "person");
        let img = annotate(&f, &[d], None).unwrap();
        // A pixel on the box top-left edge should now differ from the fill.
        let edge = img.get_pixel(10, 10);
        let bg = Rgb([30, 30, 30]);
        assert_ne!(edge, &bg, "box edge pixel should be drawn, not background");
    }

    #[test]
    fn annotate_box_at_image_edge_does_not_panic() {
        // Box flush with right/bottom edge — thickness inset must clamp.
        let f = rgb_frame(32, 24, 40);
        let d = det(28, 20, 8, 8, 0.5, "car");
        let _ = annotate(&f, &[d], None).unwrap();
    }

    #[test]
    fn encode_jpeg_returns_valid_jpeg() {
        let img = RgbImage::from_pixel(8, 8, Rgb([128, 128, 128]));
        let bytes = encode_jpeg(img).unwrap();
        // JPEG SOI marker.
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8]);
        // EOI marker at the end.
        assert_eq!(&bytes[bytes.len() - 2..], &[0xFF, 0xD9]);
    }

    #[test]
    fn frame_writer_creates_dirs_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut writer = FrameWriter::new(tmp.path(), 1).unwrap();

        let f = rgb_frame(32, 24, 50);
        let dets = vec![det(5, 5, 10, 10, 0.8, "car")];

        let path = writer.save(&f, &dets).unwrap().expect("should save");
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("seq_000001.jpg"));

        // JSONL exists and has one line.
        let jsonl = tmp.path().join("detections.jsonl");
        assert!(jsonl.exists());
        let content = fs::read_to_string(&jsonl).unwrap();
        assert_eq!(content.lines().count(), 1);
        assert!(content.contains("\"car\""));
        assert!(content.contains("0.8"));
    }

    #[test]
    fn frame_writer_throttles_every_n() {
        let tmp = tempfile::tempdir().unwrap();
        let mut writer = FrameWriter::new(tmp.path(), 3).unwrap();
        let f = rgb_frame(16, 12, 10);

        // counter is incremented BEFORE the check, so:
        // counter 1: 1%3=1 skip; 2: 2%3=2 skip; 3: 3%3=0 SAVE;
        // 4: 4%3=1 skip; 5: skip; 6: 6%3=0 SAVE.  → 2 saves out of 6.
        let mut saved = 0;
        for _ in 0..6 {
            if writer.save(&f, &[]).unwrap().is_some() {
                saved += 1;
            }
        }
        assert_eq!(saved, 2, "expected 2 saves out of 6 with every_n=3");
        assert_eq!(writer.saved_count(), 2);
    }

    #[test]
    fn frame_writer_jsonl_grows_append() {
        let tmp = tempfile::tempdir().unwrap();
        let mut writer = FrameWriter::new(tmp.path(), 1).unwrap();
        let f = rgb_frame(16, 12, 10);

        writer.save(&f, &[]).unwrap();
        writer.save(&f, &[]).unwrap();
        writer.save(&f, &[]).unwrap();

        let jsonl = tmp.path().join("detections.jsonl");
        let content = fs::read_to_string(&jsonl).unwrap();
        assert_eq!(content.lines().count(), 3);
    }

    #[test]
    fn with_font_path_missing_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let res = FrameWriter::new(tmp.path(), 1)
            .unwrap()
            .with_font_path("/nonexistent/font.ttf");
        assert!(res.is_err());
    }

    #[test]
    fn has_font_false_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let w = FrameWriter::new(tmp.path(), 1).unwrap();
        assert!(!w.has_font());
    }
}
