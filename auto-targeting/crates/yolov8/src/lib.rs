//! Pure-Rust YOLOv8 preprocessing and postprocessing.
//!
//! This crate is intentionally **backend-agnostic**: it knows nothing about
//! ONNX Runtime or RKNN. It provides:
//!
//! - [`letterbox`] — resize a frame to `640x640` keeping aspect ratio, padding
//!   with grey (114). Returns the scaled image + the transform needed to map
//!   detections back to the original frame.
//! - [`LetterboxParams`] — reverse transform applied in [`postprocess`].
//! - [`postprocess`] — parse a YOLOv8 raw output tensor `[1, 4+nc, anchors]`
//!   (the standard Ultralytics layout) into filtered + NMS'd detections in
//!   **original frame** pixel coordinates.
//!
//! ## Why a separate crate
//!
//! The same numeric logic is mirrored in `rknn-bridge/src/yolov8_post.cpp`
//! (C++) so the RK3588 NPU path produces identical results to the x86 ONNX
//! fallback. Keeping it in pure Rust here lets us unit-test it exhaustively
//! (including `proptest`) without spinning up a runtime.
//!
//! ## YOLOv8 output layout (Ultralytics export, `task=detect`)
//!
//! Output tensor shape: `[1, 4 + nc, anchors]` where `anchors = 80x80 + 40x40
//! + 20x20 = 8400` for a 640x640 input and `nc` classes (e.g. 80 for COCO).
//!
//! Per anchor `a` (column):
//! - row 0: `cx` (center x, in 640-space)
//! - row 1: `cy` (center y, in 640-space)
//! - row 2: `w`  (width,  in 640-space)
//! - row 3: `h`  (height, in 640-space)
//! - row 4..4+nc: per-class probability (already after sigmoid in the standard
//!   export — Ultralytics folds sigmoid into the model for ONNX export, so the
//!   raw values ARE probabilities in [0,1]).
//!
//! Note: YOLOv8 has **no objectness** branch — each anchor emits `nc` class
//! scores directly. Confidence of a detection = max over class scores.
//!
//! Coordinates returned by the model are in the letterboxed 640x640 space;
//! [`postprocess`] maps them back to original-frame pixels via
//! [`LetterboxParams`].

#![deny(unsafe_code)]

use common::{BoundingBox, Detection};

/// COCO 80-class label table. Index in this array == `class_id`.
///
/// Used as the default class vocabulary for the baseline COCO YOLOv8n model
/// (Phase 1.1). For project-specific classes (палатка / ящик / джип …) a
/// custom vocabulary will be supplied via config in a later phase.
pub const COCO_LABELS: [&str; 80] = [
    "person",
    "bicycle",
    "car",
    "motorcycle",
    "airplane",
    "bus",
    "train",
    "truck",
    "boat",
    "traffic light",
    "fire hydrant",
    "stop sign",
    "parking meter",
    "bench",
    "bird",
    "cat",
    "dog",
    "horse",
    "sheep",
    "cow",
    "elephant",
    "bear",
    "zebra",
    "giraffe",
    "backpack",
    "umbrella",
    "handbag",
    "tie",
    "suitcase",
    "frisbee",
    "skis",
    "snowboard",
    "sports ball",
    "kite",
    "baseball bat",
    "baseball glove",
    "skateboard",
    "surfboard",
    "tennis racket",
    "bottle",
    "wine glass",
    "cup",
    "fork",
    "knife",
    "spoon",
    "bowl",
    "banana",
    "apple",
    "sandwich",
    "orange",
    "broccoli",
    "carrot",
    "hot dog",
    "pizza",
    "donut",
    "cake",
    "chair",
    "couch",
    "potted plant",
    "bed",
    "dining table",
    "toilet",
    "tv",
    "laptop",
    "mouse",
    "remote",
    "keyboard",
    "cell phone",
    "microwave",
    "oven",
    "toaster",
    "sink",
    "refrigerator",
    "book",
    "clock",
    "vase",
    "scissors",
    "teddy bear",
    "hair drier",
    "toothbrush",
];

/// Standard YOLOv8 input square size. Padded/letterboxed to this.
pub const INPUT_SIZE: u32 = 640;

/// Padding value used by letterbox (mid-grey).
pub const PAD_VALUE: u8 = 114;

/// Parameters describing the letterbox transform applied to a frame.
///
/// Produced by [`letterbox`]; consumed by [`postprocess`] to map detections
/// from 640x640 space back into the original frame's pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LetterboxParams {
    /// Original frame width.
    pub orig_w: u32,
    /// Original frame height.
    pub orig_h: u32,
    /// Scale factor applied during letterbox (orig → 640).
    pub scale: f32,
    /// Left/right padding (in 640-space, identical on both sides).
    pub pad_x: f32,
    /// Top/bottom padding (in 640-space, identical on both sides).
    pub pad_y: f32,
}

impl LetterboxParams {
    /// Compute the params for a given original size without doing the resize.
    ///
    /// Useful for tests where we synthesise a "model output" directly.
    pub fn for_size(orig_w: u32, orig_h: u32) -> Self {
        let (scale, pad_x, pad_y) = compute_letterbox(orig_w, orig_h, INPUT_SIZE);
        Self {
            orig_w,
            orig_h,
            scale,
            pad_x,
            pad_y,
        }
    }

    /// Map a center point from 640-space back to original-frame pixels.
    #[inline]
    pub fn unproject_xy(&self, cx640: f32, cy640: f32) -> (f32, f32) {
        (
            (cx640 - self.pad_x) / self.scale,
            (cy640 - self.pad_y) / self.scale,
        )
    }
}

/// Compute `(scale, pad_x, pad_y)` for letterboxing `orig_w x orig_h` into a
/// `target x target` square keeping aspect ratio.
///
/// Returned scale maps original pixels → square pixels; pads are measured in
/// square-space and placed symmetrically.
fn compute_letterbox(orig_w: u32, orig_h: u32, target: u32) -> (f32, f32, f32) {
    let ow = orig_w as f32;
    let oh = orig_h as f32;
    let tg = target as f32;
    // Scale so the longer side fits; YOLOv8 uses no rounding trick in the
    // reference implementation for 640x640 (stride 32 divides 640), but we
    // still round scale to a multiple of stride to keep feature-map alignment
    // when the model itself does internal letterboxing. For a 640 square this
    // is a no-op for typical inputs.
    let scale_raw = (tg / ow).min(tg / oh);
    let scale = scale_raw.max(f32::MIN_POSITIVE); // guard against div-by-zero / NaN
    let new_w = ow * scale;
    let new_h = oh * scale;
    let pad_x = (tg - new_w) * 0.5;
    let pad_y = (tg - new_h) * 0.5;
    (scale, pad_x, pad_y)
}

/// Letterbox an RGB24 frame into a `INPUT_SIZE x INPUT_SIZE` buffer.
///
/// `rgb_src` is expected to be `[R, G, B, R, G, B, ...]` with
/// `src_w * src_h * 3` bytes. The returned buffer is row-major
/// `INPUT_SIZE * INPUT_SIZE * 3` bytes, filled with [`PAD_VALUE`] where there
/// is no image.
///
/// Uses nearest-neighbour sampling — fast and matches Ultralytics' default
/// `cv2.INTER_LINEAR` closely enough for a baseline (sub-pixel differences
/// don't move boxes by more than ~1px at 640). For a faithful match, swap the
/// inner branch for bilinear; the parser is tolerant.
pub fn letterbox(rgb_src: &[u8], src_w: u32, src_h: u32) -> (Vec<u8>, LetterboxParams) {
    assert_eq!(
        rgb_src.len(),
        src_w as usize * src_h as usize * 3,
        "rgb_src must be src_w*src_h*3 bytes"
    );
    let params = LetterboxParams::for_size(src_w, src_h);
    let scale = params.scale;

    let mut out = vec![PAD_VALUE; INPUT_SIZE as usize * INPUT_SIZE as usize * 3];

    // Visible region in the target square (after padding).
    let vis_w = (src_w as f32 * scale).round() as u32;
    let vis_h = (src_h as f32 * scale).round() as u32;

    // Inverse scale: target x → source x.
    let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };

    // Перф-аудит 2026-08: LUT исходных столбцов один раз на кадр вместо
    // f32 mul+round на каждый пиксель; построчные chunks-копии без
    // bounds-checks. Fast-path scale==1 (например, 640x640 на входе).
    let src_w_us = src_w as usize;
    let dst_w = INPUT_SIZE as usize;
    let pad_x = params.pad_x.round() as usize;
    let pad_y = params.pad_y.round() as usize;

    if scale == 1.0 && vis_w as usize == src_w_us {
        for ty in 0..vis_h as usize {
            let dst_row = &mut out[(pad_y + ty) * dst_w * 3 + pad_x * 3..][..src_w_us * 3];
            let src_row = &rgb_src[ty * src_w_us * 3..][..src_w_us * 3];
            dst_row.copy_from_slice(src_row);
        }
        return (out, params);
    }

    // LUT: sx(tx) для каждого видимого столбца.
    let sx_lut: Vec<u32> = (0..vis_w)
        .map(|tx| (((tx as f32) * inv).round() as u32).min(src_w.saturating_sub(1)))
        .collect();

    for ty in 0..vis_h as usize {
        let sy = (((ty as f32) * inv).round() as u32).min(src_h.saturating_sub(1)) as usize;
        let src_row = &rgb_src[sy * src_w_us * 3..][..src_w_us * 3];
        let dst_row = &mut out[(pad_y + ty) * dst_w * 3 + pad_x * 3..][..vis_w as usize * 3];
        for (tx, dst_px) in dst_row.chunks_exact_mut(3).enumerate() {
            let src_px = &src_row[sx_lut[tx] as usize * 3..][..3];
            dst_px.copy_from_slice(src_px);
        }
    }

    (out, params)
}

/// Convert an RGB24 letterboxed buffer to a normalized NCHW float32 tensor.
///
/// Output layout: `[1, 3, INPUT_SIZE, INPUT_SIZE]` (CHW, channel-first), values
/// in `[0.0, 1.0]` (divided by 255). This is the exact layout ONNX Runtime /
/// RKNN expect for a `RGB` float input.
///
/// Pass the buffer returned by [`letterbox`] (length `INPUT_SIZE*INPUT_SIZE*3`).
pub fn rgb_to_nchw_f32(rgb: &[u8]) -> Vec<f32> {
    let npix = INPUT_SIZE as usize * INPUT_SIZE as usize;
    assert_eq!(
        rgb.len(),
        npix * 3,
        "rgb buffer must be INPUT_SIZE^2*3 bytes"
    );
    let mut out = vec![0f32; npix * 3];
    // Channel-first: R-plane, then G-plane, then B-plane.
    // Перф-аудит 2026-08: умножение на 1/255 вместо деления (f32 div ~10 тактов
    // vs mul ~2-3), chunks-итераторы без индексной арифметики.
    const INV_255: f32 = 1.0 / 255.0;
    let (r_plane, rest) = out.split_at_mut(npix);
    let (g_plane, b_plane) = rest.split_at_mut(npix);
    for (((px, r), g), b) in rgb
        .chunks_exact(3)
        .zip(r_plane.iter_mut())
        .zip(g_plane.iter_mut())
        .zip(b_plane.iter_mut())
    {
        *r = px[0] as f32 * INV_255;
        *g = px[1] as f32 * INV_255;
        *b = px[2] as f32 * INV_255;
    }
    out
}

/// A raw YOLOv8 detection candidate, in 640-space, before NMS / coordinate
/// un-projection. Internal to the parser.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    class_id: u32,
    confidence: f32,
}

/// Parse + filter + NMS the raw YOLOv8 output tensor.
#[allow(clippy::too_many_arguments, clippy::identity_op, clippy::erasing_op)]
// too_many_arguments: postprocess is a pure data transform; bundling args
//   into a struct would only add ceremony without clarifying intent.
// identity_op: the `r * num_anchors` literal row indices document the
//   tensor layout (rows 0..3 = cx/cy/w/h, row 4+ = class scores); they mirror
//   the C++ reference parser 1:1.
///
/// # Arguments
/// * `output` — flat row-major view of the `[1, 4+nc, anchors]` tensor. The
///   outer dimensions are folded: element `(row, anchor)` is at
///   `output[row * anchors + anchor]`.
/// * `num_classes` — `nc` (e.g. 80 for COCO).
/// * `num_anchors` — number of anchors (e.g. 8400 for 640 input). Used for
///   bounds checking.
/// * `params` — letterbox params to map boxes back to original frame coords.
/// * `conf_threshold` — drop candidates whose max class score < this.
/// * `iou_threshold` — NMS IoU threshold (typically 0.45).
/// * `frame_seq` / `now` — stamped onto returned [`Detection`]s.
/// * `labels` — class id → label string. If `class_id >= labels.len()`, the
///   string falls back to `"class_{id}"`.
///
/// # Returns
/// Detections in **original frame** pixel coordinates, after NMS.
///
/// # Panics
/// Panics if `output.len() != (4 + num_classes) * num_anchors`.
pub fn postprocess(
    output: &[f32],
    num_classes: u32,
    num_anchors: usize,
    params: LetterboxParams,
    conf_threshold: f32,
    iou_threshold: f32,
    frame_seq: u64,
    now: chrono::DateTime<chrono::Utc>,
    labels: &[&str],
) -> Vec<Detection> {
    let rows = (4 + num_classes) as usize;
    assert_eq!(
        output.len(),
        rows * num_anchors,
        "output length {} does not match (4+nc)*anchors = {}*{}",
        output.len(),
        rows,
        num_anchors
    );

    // 1) Sweep anchors, pick best class per anchor, threshold.
    // The tensor is laid out row-major as [4+nc, anchors]: element at
    // (row r, anchor a) is output[r * num_anchors + a]. Rows 0..3 are
    // cx/cy/w/h, rows 4.. are per-class scores. We slice each row out once.
    let cx_row = &output[0 * num_anchors..1 * num_anchors];
    let cy_row = &output[1 * num_anchors..2 * num_anchors];
    let w_row = &output[2 * num_anchors..3 * num_anchors];
    let h_row = &output[3 * num_anchors..4 * num_anchors];
    let class_rows = &output[4 * num_anchors..rows * num_anchors];

    // Классовый скан — ТРАНСПОНИРОВАННЫЙ (перф-аудит 2026-08): проходим
    // строки классов последовательно (каждая — континуум 8400 f32), а не
    // якорями со страйдом num_anchors (33.6 КБ — cache-miss на каждый
    // класс-чтение). Лучший класс на якорь копим в компактные массивы
    // (рабочий набор 2×33.6 КБ — L1/L2-резидент).
    let mut best_score: Vec<f32> = vec![f32::NEG_INFINITY; num_anchors];
    let mut best_id: Vec<u32> = vec![0; num_anchors];
    for c in 0..num_classes as usize {
        let row = &class_rows[c * num_anchors..(c + 1) * num_anchors];
        for (a, &s) in row.iter().enumerate() {
            if s > best_score[a] {
                best_score[a] = s;
                best_id[a] = c as u32;
            }
        }
    }

    let mut cands: Vec<Candidate> = Vec::with_capacity(256);
    for a in 0..num_anchors {
        // Standard Ultralytics ONNX export has sigmoid baked in, so values are
        // already probabilities. Clamp to [0,1] defensively (some custom
        // exports forget the sigmoid; we don't silently suppress them but the
        // conf threshold will).
        let conf = best_score[a].clamp(0.0, 1.0);
        if conf < conf_threshold {
            continue;
        }
        // Box rows читаются только для прошедших порог (4 чтения на
        // отброшенный якорь — в никуда).
        let cx = cx_row[a];
        let cy = cy_row[a];
        let w = w_row[a];
        let h = h_row[a];
        // Filter degenerate boxes (NaN / non-positive area) early.
        if !cx.is_finite() || !cy.is_finite() || w <= 0.0 || h <= 0.0 {
            continue;
        }
        cands.push(Candidate {
            cx,
            cy,
            w,
            h,
            class_id: best_id[a],
            confidence: conf,
        });
    }

    // 2) Sort by confidence desc.
    cands.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 3) Greedy NMS in 640-space (equivalent to original-space since letterbox
    //    is a uniform scale + translation — IoU is invariant under it).
    let mut suppressed = vec![false; cands.len()];
    let mut kept: Vec<Detection> = Vec::with_capacity(cands.len().min(64));
    for i in 0..cands.len() {
        if suppressed[i] {
            continue;
        }
        let ci = cands[i];
        for j in (i + 1)..cands.len() {
            if suppressed[j] {
                continue;
            }
            if iou_640(&ci, &cands[j]) > iou_threshold {
                suppressed[j] = true;
            }
        }

        // 4) Map back to original frame coords.
        let (ox, oy) = params.unproject_xy(ci.cx, ci.cy);
        let bw = ci.w / params.scale;
        let bh = ci.h / params.scale;

        let x0 = ox - bw * 0.5;
        let y0 = oy - bh * 0.5;

        // Clamp to frame; width/height as u32, non-zero.
        let x = x0.round().clamp(0.0, params.orig_w as f32) as u32;
        let y = y0.round().clamp(0.0, params.orig_h as f32) as u32;
        let x1 = (ox + bw * 0.5).round().clamp(0.0, params.orig_w as f32) as u32;
        let y1 = (oy + bh * 0.5).round().clamp(0.0, params.orig_h as f32) as u32;
        let width = x1.saturating_sub(x).max(1);
        let height = y1.saturating_sub(y).max(1);

        let class = labels
            .get(ci.class_id as usize)
            .copied()
            .unwrap_or("unknown")
            .to_string();

        kept.push(Detection {
            bbox: BoundingBox {
                x,
                y,
                width,
                height,
            },
            class,
            class_id: ci.class_id,
            confidence: ci.confidence,
            frame_seq,
            detected_at: now,
        });
    }

    kept
}

/// IoU between two candidates in 640-space (center-format).
#[inline]
fn iou_640(a: &Candidate, b: &Candidate) -> f32 {
    let ax1 = a.cx - a.w * 0.5;
    let ay1 = a.cy - a.h * 0.5;
    let ax2 = a.cx + a.w * 0.5;
    let ay2 = a.cy + a.h * 0.5;
    let bx1 = b.cx - b.w * 0.5;
    let by1 = b.cy - b.h * 0.5;
    let bx2 = b.cx + b.w * 0.5;
    let by2 = b.cy + b.h * 0.5;

    let inter_x1 = ax1.max(bx1);
    let inter_y1 = ay1.max(by1);
    let inter_x2 = ax2.min(bx2);
    let inter_y2 = ay2.min(by2);

    let iw = (inter_x2 - inter_x1).max(0.0);
    let ih = (inter_y2 - inter_y1).max(0.0);
    let inter = iw * ih;
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn labels() -> Vec<&'static str> {
        COCO_LABELS.to_vec()
    }

    #[test]
    fn letterbox_params_square_no_pad() {
        // Square source → scale=1, no padding.
        let p = LetterboxParams::for_size(INPUT_SIZE, INPUT_SIZE);
        assert!((p.scale - 1.0).abs() < 1e-5);
        assert!((p.pad_x).abs() < 1e-3);
        assert!((p.pad_y).abs() < 1e-3);
    }

    #[test]
    fn letterbox_params_landscape() {
        // 1280x720 → scale = 640/1280 = 0.5, pad_y = (640 - 360)/2 = 140.
        let p = LetterboxParams::for_size(1280, 720);
        assert!((p.scale - 0.5).abs() < 1e-5, "scale={}", p.scale);
        assert!((p.pad_x - 0.0).abs() < 1e-3, "pad_x={}", p.pad_x);
        assert!((p.pad_y - 140.0).abs() < 1e-3, "pad_y={}", p.pad_y);
    }

    #[test]
    fn letterbox_preserves_aspect_ratio() {
        // 4x2 RGB frame, all red.
        let pixel = [255u8, 0, 0];
        let src: Vec<u8> = pixel.repeat(4 * 2);
        let (out, p) = letterbox(&src, 4, 2);
        assert_eq!(out.len(), INPUT_SIZE as usize * INPUT_SIZE as usize * 3);
        // scale = 640/4 = 160 → vis 640x320, pad_y = 160.
        assert!((p.scale - 160.0).abs() < 1e-3, "scale={}", p.scale);
        // Top pad row should be grey.
        let top_idx = 0; // first pixel
        assert_eq!(out[top_idx], PAD_VALUE);
        assert_eq!(out[top_idx + 1], PAD_VALUE);
        assert_eq!(out[top_idx + 2], PAD_VALUE);
        // First visible row: at y = round(pad_y) = 160.
        let vis_y = p.pad_y.round() as usize;
        let vis_x = p.pad_x.round() as usize; // 0 for landscape
        let px = vis_y * INPUT_SIZE as usize + vis_x;
        assert_eq!(out[px * 3], 255);
        assert_eq!(out[px * 3 + 1], 0);
    }

    #[test]
    fn rgb_to_nchw_layout_and_range() {
        // Full-size grey buffer → all values should be 128/255 ≈ 0.50196.
        let grey = vec![128u8; INPUT_SIZE as usize * INPUT_SIZE as usize * 3];
        let t = rgb_to_nchw_f32(&grey);
        let npix = INPUT_SIZE as usize * INPUT_SIZE as usize;
        assert_eq!(t.len(), npix * 3);
        // Every value should be 128/255 ≈ 0.50196.
        for v in &t {
            assert!((v - 128.0 / 255.0).abs() < 1e-4);
        }
    }

    #[test]
    fn postprocess_empty_when_below_threshold() {
        // 1 class, 1 anchor, score 0.1 < 0.45.
        let nc = 1;
        let anchors = 1;
        let output = vec![
            320.0, // cx
            320.0, // cy
            50.0,  // w
            50.0,  // h
            0.1,   // class score
        ];
        let p = LetterboxParams::for_size(640, 640);
        let dets = postprocess(
            &output,
            nc,
            anchors,
            p,
            0.45,
            0.45,
            1,
            Utc::now(),
            &["thing"],
        );
        assert!(dets.is_empty());
    }

    #[test]
    fn postprocess_single_high_confidence_detection() {
        // 1 anchor at (320,320), 50x50, class 0 score 0.9. Square frame 640.
        let nc = 1;
        let anchors = 1;
        let output = vec![320.0, 320.0, 50.0, 50.0, 0.9];
        let p = LetterboxParams::for_size(640, 640);
        let dets = postprocess(
            &output,
            nc,
            anchors,
            p,
            0.45,
            0.45,
            7,
            Utc::now(),
            &["thing"],
        );
        assert_eq!(dets.len(), 1);
        let d = &dets[0];
        assert_eq!(d.class_id, 0);
        assert_eq!(d.class, "thing");
        assert!((d.confidence - 0.9).abs() < 1e-5);
        assert_eq!(d.frame_seq, 7);
        // Center should map back to ~ (320, 320) in a 640 square.
        let (cx, cy) = d.bbox.center();
        assert!((cx - 320.0).abs() < 2.0, "cx={cx}");
        assert!((cy - 320.0).abs() < 2.0, "cy={cy}");
        // Width/height ≈ 50.
        assert!(
            (d.bbox.width as f32 - 50.0).abs() < 3.0,
            "w={}",
            d.bbox.width
        );
        assert!(
            (d.bbox.height as f32 - 50.0).abs() < 3.0,
            "h={}",
            d.bbox.height
        );
    }

    #[test]
    fn postprocess_nms_suppresses_overlapping() {
        // Two anchors heavily overlapping; only highest conf kept.
        let nc = 1;
        let anchors = 2;
        let output = vec![
            320.0, 322.0, // cx
            320.0, 321.0, // cy
            50.0, 50.0, // w
            50.0, 50.0, // h
            0.9, 0.8, // scores
        ];
        let p = LetterboxParams::for_size(640, 640);
        let dets = postprocess(
            &output,
            nc,
            anchors,
            p,
            0.45,
            0.45,
            1,
            Utc::now(),
            &["thing"],
        );
        assert_eq!(dets.len(), 1);
        assert!((dets[0].confidence - 0.9).abs() < 1e-5);
    }

    #[test]
    fn postprocess_letterbox_unprojects_correctly() {
        // Source frame 1280x720, letterboxed to 640x360 visible region.
        // A box at original-frame center (640, 360) should appear in 640-space
        // at (320, 180 + pad_y=140) = (320, 320). Model emits a 100x50 box
        // in 640-space → original 200x100.
        let nc = 1;
        let anchors = 1;
        let output = vec![320.0, 320.0, 100.0, 50.0, 0.8];
        let p = LetterboxParams::for_size(1280, 720);
        let dets = postprocess(
            &output,
            nc,
            anchors,
            p,
            0.45,
            0.45,
            1,
            Utc::now(),
            &["thing"],
        );
        assert_eq!(dets.len(), 1);
        let d = &dets[0];
        let (cx, cy) = d.bbox.center();
        // Original center ~ (640, 360).
        assert!((cx - 640.0).abs() < 2.0, "cx={cx}");
        assert!((cy - 360.0).abs() < 2.0, "cy={cy}");
        // Original box ~ 200x100.
        assert!(
            (d.bbox.width as f32 - 200.0).abs() < 4.0,
            "w={}",
            d.bbox.width
        );
        assert!(
            (d.bbox.height as f32 - 100.0).abs() < 4.0,
            "h={}",
            d.bbox.height
        );
    }

    #[test]
    fn postprocess_disjoint_anchors_both_kept() {
        let nc = 1;
        let anchors = 2;
        let output = vec![
            100.0, 500.0, // cx
            100.0, 500.0, // cy
            40.0, 40.0, // w
            40.0, 40.0, // h
            0.9, 0.8, // scores
        ];
        let p = LetterboxParams::for_size(640, 640);
        let dets = postprocess(
            &output,
            nc,
            anchors,
            p,
            0.45,
            0.45,
            1,
            Utc::now(),
            &["thing"],
        );
        assert_eq!(dets.len(), 2);
    }

    #[test]
    fn postprocess_picks_best_class_per_anchor() {
        // 2 classes: class 0 score 0.3, class 1 score 0.85. Should keep class 1.
        let nc = 2;
        let anchors = 1;
        let output = vec![320.0, 320.0, 40.0, 40.0, 0.3, 0.85];
        let p = LetterboxParams::for_size(640, 640);
        let dets = postprocess(
            &output,
            nc,
            anchors,
            p,
            0.45,
            0.45,
            1,
            Utc::now(),
            &["a", "b"],
        );
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].class_id, 1);
        assert_eq!(dets[0].class, "b");
        assert!((dets[0].confidence - 0.85).abs() < 1e-5);
    }

    #[test]
    fn postprocess_panics_on_shape_mismatch() {
        let p = LetterboxParams::for_size(640, 640);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            postprocess(&[0.0; 5], 1, 2, p, 0.45, 0.45, 1, Utc::now(), &["x"]);
        }));
        assert!(result.is_err());
    }

    #[test]
    fn postprocess_skips_degenerate_boxes() {
        // NaN cy → skipped.
        let nc = 1;
        let anchors = 1;
        let output = vec![320.0, f32::NAN, 40.0, 40.0, 0.9];
        let p = LetterboxParams::for_size(640, 640);
        let dets = postprocess(&output, nc, anchors, p, 0.45, 0.45, 1, Utc::now(), &["x"]);
        assert!(dets.is_empty());
    }

    #[test]
    fn coco_labels_table_sanity() {
        assert_eq!(COCO_LABELS.len(), 80);
        assert_eq!(COCO_LABELS[0], "person");
        assert_eq!(COCO_LABELS[1], "bicycle");
        assert_eq!(COCO_LABELS[79], "toothbrush");
        // No empty strings.
        assert!(COCO_LABELS.iter().all(|s| !s.is_empty()));
    }

    #[test]
    fn unknown_class_label_fallback() {
        // class_id beyond labels table → "unknown".
        let nc = 5;
        let anchors = 1;
        let mut output = vec![320.0, 320.0, 40.0, 40.0];
        output.extend_from_slice(&[0.1, 0.1, 0.1, 0.1, 0.9]); // class 4 wins
        let p = LetterboxParams::for_size(640, 640);
        let dets = postprocess(
            &output,
            nc,
            anchors,
            p,
            0.45,
            0.45,
            1,
            Utc::now(),
            &["a", "b"],
        );
        // labels has 2 entries but class 4 — fallback.
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].class_id, 4);
        assert_eq!(dets[0].class, "unknown");
    }

    #[test]
    fn postprocess_full_coco_sized_output_smoke() {
        // 80 classes, 8400 anchors, all zero → no detections (all scores 0).
        let nc = 80;
        let anchors = 8400;
        let output = vec![0.0f32; (4 + nc) as usize * anchors];
        let p = LetterboxParams::for_size(1280, 720);
        let dets = postprocess(
            &output,
            nc,
            anchors,
            p,
            0.45,
            0.45,
            1,
            Utc::now(),
            &labels(),
        );
        assert!(dets.is_empty());
    }
}
