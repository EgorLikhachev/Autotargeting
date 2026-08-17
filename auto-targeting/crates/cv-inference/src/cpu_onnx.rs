//! CPU inference backend via ONNX Runtime (`ort` crate).
//!
//! Only compiled when the `cpu-onnx` feature is enabled. This is the x86
//! development / fallback path described in ADR-0001 — the RK3588 NPU path
//! (`RknnBridgeClient`) is the production target, but the CPU path lets us
//! run the **full** minimal loop (camera → model → detections) on a dev
//! machine without an NPU, and is what Phase 1.1's baseline numbers are first
//! captured on.
//!
//! ## Pipeline
//!
//! 1. `Frame` (RGB24 or NV12) → flat RGB24 bytes.
//! 2. [`yolov8::letterbox`] → 640×640 RGB, keeping aspect ratio.
//! 3. [`yolov8::rgb_to_nchw_f32`] → normalized NCHW float tensor.
//! 4. `ort` `session.run` → raw output `[1, 4+nc, anchors]` float32.
//! 5. [`yolov8::postprocess`] → `Vec<Detection>` in original frame coords.
//!
//! The ONNX model is assumed to be a standard Ultralytics YOLOv8 detect
//! export: input `[1,3,640,640]` float32 (first input), output
//! `[1,4+nc,8400]` float32 (first output). Names are read from the model.
//!
//! ## ort API notes (v2.0.0-rc.13)
//!
//! - `Environment::current()` lazily creates a default environment — no need
//!   to call `ort::init()` ourselves for a single-process app.
//! - `Session` is `&mut self` for `run`, so `infer` is `&mut self` too.
//! - Output extraction: `value.try_extract_tensor::<f32>()?` returns
//!   `(&Shape, &[f32])` — a borrowed flat slice, zero-copy.

use crate::backend::{InferenceBackend, InferenceError, InferenceResult};
use async_trait::async_trait;
use chrono::Utc;
use common::{Detection, Frame, PixelFormat};
use ndarray::Array4;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::Session;
use ort::value::Tensor;
use yolov8::{letterbox, postprocess, rgb_to_nchw_f32, COCO_LABELS, INPUT_SIZE};

/// Anchors for a 640×640 YOLOv8 input (80² + 40² + 20²).
const ANCHORS_640: usize = 8400;

/// CPU ONNX Runtime inference backend.
///
/// Construct with [`CpuInferenceBackend::new`], then call [`InferenceBackend::init`]
/// before [`InferenceBackend::infer`].
pub struct CpuInferenceBackend {
    pub model_path: String,
    pub confidence_threshold: f32,
    pub iou_threshold: f32,
    /// Class vocabulary. Defaults to COCO 80-class table. Override for custom
    /// models (Phase 1.2 narrow classes).
    pub labels: Vec<String>,
    session: Option<Session>,
    /// Имя входа модели (кэш — было String-аллокацией на каждый кадр;
    /// перф-аудит 2026-08).
    input_name: Option<String>,
}

impl CpuInferenceBackend {
    /// Create with COCO labels and the standard thresholds.
    pub fn new(model_path: impl Into<String>) -> Self {
        Self {
            model_path: model_path.into(),
            confidence_threshold: 0.45,
            iou_threshold: 0.45,
            labels: COCO_LABELS.iter().map(|s| s.to_string()).collect(),
            session: None,
            input_name: None,
        }
    }

    /// Override confidence threshold (config `[inference] confidence_threshold`).
    pub fn with_confidence_threshold(mut self, t: f32) -> Self {
        self.confidence_threshold = t;
        self
    }

    /// Override NMS IoU threshold (config `[inference] nms_threshold`).
    pub fn with_iou_threshold(mut self, t: f32) -> Self {
        self.iou_threshold = t;
        self
    }

    /// Override class label table.
    pub fn with_labels(mut self, labels: Vec<String>) -> Self {
        self.labels = labels;
        self
    }

    /// Run the full preprocessing + inference + postprocessing on one frame.
    fn infer_sync(&mut self, frame: &Frame) -> InferenceResult<Vec<Detection>> {
        let session = self
            .session
            .as_mut()
            .ok_or(InferenceError::ModelNotLoaded)?;

        // 1) Frame → RGB24 bytes.
        let rgb = frame_to_rgb24(frame)?;

        // 2) Letterbox to 640×640.
        let (letterboxed, params) = letterbox(&rgb, frame.metadata.width, frame.metadata.height);

        // 3) NCHW float tensor.
        let tensor_data = rgb_to_nchw_f32(&letterboxed);
        let input_array = Array4::from_shape_vec(
            (1usize, 3, INPUT_SIZE as usize, INPUT_SIZE as usize),
            tensor_data,
        )
        .map_err(|e| InferenceError::Inference(format!("tensor reshape: {e}")))?;

        // Wrap the ndarray into an ort Value (Tensor<f32>).
        let input_value = Tensor::from_array(input_array)
            .map_err(|e| InferenceError::Inference(format!("ort tensor from array: {e}")))?;

        // 4) Build the named inputs ort expects. Имя входа закэшировано
        //    при init (перф-аудит 2026-08: без String-аллокации на кадр).
        let input_name = self
            .input_name
            .as_deref()
            .ok_or_else(|| InferenceError::Inference("backend not initialized".into()))?;

        // `inputs!` expands to a Vec<(Cow<str>, SessionInputValue)>. It does
        // not return a Result in ort 2.0.0-rc.13.
        let inputs = ort::inputs! {
            input_name => input_value
        };

        // 5) Run inference.
        let outputs = session
            .run(inputs)
            .map_err(|e| InferenceError::Inference(format!("ort run: {e}")))?;

        // 6) Extract first output as flat f32 slice.
        if outputs.len() == 0 {
            return Err(InferenceError::Inference(
                "model produced no outputs".into(),
            ));
        }
        // Index by position; SessionOutputs also supports indexing by name.
        let out_value = &outputs[0usize];

        let (_shape, floats): (&ort::value::Shape, &[f32]) = out_value
            .try_extract_tensor::<f32>()
            .map_err(|e| InferenceError::Inference(format!("ort extract f32: {e}")))?;

        // 7) Validate shape and postprocess. We assume the canonical 640-input
        //    layout: (4 + nc) × 8400, nc inferred as rows-4.
        if floats.len() % ANCHORS_640 != 0 || floats.len() / ANCHORS_640 < 5 {
            return Err(InferenceError::Inference(format!(
                "unexpected YOLOv8 output: {} floats (expected rows*{ANCHORS_640}, rows>=5)",
                floats.len()
            )));
        }
        let rows = floats.len() / ANCHORS_640;
        let num_classes = (rows - 4) as u32;

        let labels_refs: Vec<&str> = self.labels.iter().map(|s| s.as_str()).collect();
        Ok(postprocess(
            floats,
            num_classes,
            ANCHORS_640,
            params,
            self.confidence_threshold,
            self.iou_threshold,
            frame.metadata.seq,
            Utc::now(),
            &labels_refs,
        ))
    }
}

#[async_trait]
impl InferenceBackend for CpuInferenceBackend {
    async fn init(&mut self) -> InferenceResult<()> {
        let path = self.model_path.clone();
        // ort does disk I/O + native lib init — block-friendly but cheap
        // enough (tens of ms) to do inline.
        let session: Session = {
            SessionBuilder::new()
                .map_err(|e| InferenceError::Inference(format!("ort builder: {e}")))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| InferenceError::Inference(format!("ort opt level: {e}")))?
                .with_intra_threads(1usize)
                .map_err(|e| InferenceError::Inference(format!("ort threads: {e}")))?
                .commit_from_file(&path)
                .map_err(|e| InferenceError::Inference(format!("ort commit_from_file: {e}")))?
        };

        tracing::info!(
            model = %self.model_path,
            inputs = session.inputs().len(),
            outputs = session.outputs().len(),
            "CpuInferenceBackend: ONNX model loaded"
        );
        // Кэшируем имя входа (per-frame lookup делал String-аллокацию
        // на каждый кадр — перф-аудит 2026-08).
        self.input_name = session
            .inputs()
            .first()
            .map(|i| i.name().to_string());
        self.session = Some(session);
        Ok(())
    }

    async fn infer(&mut self, frame: &Frame) -> InferenceResult<Vec<Detection>> {
        if self.session.is_none() {
            return Err(InferenceError::ModelNotLoaded);
        }
        self.infer_sync(frame)
    }

    async fn health_check(&self) -> InferenceResult<()> {
        if self.session.is_some() {
            Ok(())
        } else {
            Err(InferenceError::ModelNotLoaded)
        }
    }

    fn name(&self) -> &str {
        "CpuInferenceBackend(ONNX)"
    }
}

/// Convert a `Frame` of any supported pixel format to a flat RGB24 byte
/// buffer (`[R,G,B, R,G,B, ...]`), sized `width*height*3`.
///
/// Handles RGB24 (memcpy) and NV12 (BT.601 conversion) directly. For MJPEG /
/// YUYV, callers should decode upstream via `video-capture::convert`; here we
/// return a clear error pointing there.
fn frame_to_rgb24(frame: &Frame) -> InferenceResult<Vec<u8>> {
    let w = frame.metadata.width as usize;
    let h = frame.metadata.height as usize;
    match frame.metadata.format {
        PixelFormat::Rgb24 => {
            let expected = w * h * 3;
            if frame.data.len() != expected {
                return Err(InferenceError::Inference(format!(
                    "RGB24 frame size mismatch: got {}, expected {expected}",
                    frame.data.len()
                )));
            }
            Ok(frame.data.clone())
        }
        PixelFormat::Nv12 => {
            let expected = w * h * 3 / 2;
            if frame.data.len() != expected {
                return Err(InferenceError::Inference(format!(
                    "NV12 frame size mismatch: got {}, expected {expected}",
                    frame.data.len()
                )));
            }
            Ok(nv12_to_rgb24(&frame.data, w, h))
        }
        other => Err(InferenceError::Inference(format!(
            "CpuInferenceBackend expects RGB24 or NV12 frame; got {other:?}. \
             Decode MJPEG/YUYV upstream via video-capture::convert."
        ))),
    }
}

/// NV12 (semi-planar YUV 4:2:0) → RGB24, BT.601 full-range approximation.
///
/// Inline rather than depending on video-capture to keep cv-inference's build
/// graph simple for the cpu-onnx path.
fn nv12_to_rgb24(nv12: &[u8], w: usize, h: usize) -> Vec<u8> {
    // Перф-аудит 2026-08: integer BT.601 (×256 fixed-point) + построчные
    // chunks (без bounds-checks и div на пиксель); хрома читается на пару
    // пикселей. Расхождение с f32-эталоном ≤ 1.
    let mut out = vec![0u8; w * h * 3];
    let uv_off = w * h;
    let clamp8 = |v: i32| v.clamp(0, 255) as u8;
    for (j, (y_row, out_row)) in nv12[..w * h]
        .chunks_exact(w)
        .zip(out.chunks_exact_mut(w * 3))
        .enumerate()
    {
        let uv_row = &nv12[uv_off + (j / 2) * w..][..w];
        for (k, (y2, out6)) in y_row
            .chunks_exact(2)
            .zip(out_row.chunks_exact_mut(6))
            .enumerate()
        {
            let (u, v) = (uv_row[k * 2] as i32 - 128, uv_row[k * 2 + 1] as i32 - 128);
            let y0 = y2[0] as i32;
            let y1 = y2[1] as i32;
            out6.copy_from_slice(&[
                clamp8(y0 + ((359 * v) >> 8)),
                clamp8(y0 - ((88 * u + 183 * v) >> 8)),
                clamp8(y0 + ((454 * u) >> 8)),
                clamp8(y1 + ((359 * v) >> 8)),
                clamp8(y1 - ((88 * u + 183 * v) >> 8)),
                clamp8(y1 + ((454 * u) >> 8)),
            ]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use common::FrameMetadata;

    #[test]
    fn nv12_to_rgb_black_is_black() {
        // NV12 black: Y=0, U=V=128.
        let w = 4;
        let h = 4;
        let mut nv12 = vec![0u8; w * h * 3 / 2];
        for b in &mut nv12[..w * h] {
            *b = 0;
        }
        for b in &mut nv12[w * h..] {
            *b = 128;
        }
        let rgb = nv12_to_rgb24(&nv12, w, h);
        assert_eq!(rgb.len(), w * h * 3);
        for chunk in rgb.chunks_exact(3) {
            assert_eq!(chunk, &[0, 0, 0]);
        }
    }

    #[test]
    fn nv12_to_rgb_white_is_white() {
        // NV12 white: Y=255, U=V=128.
        let w = 2;
        let h = 2;
        let mut nv12 = vec![0u8; w * h * 3 / 2];
        for b in &mut nv12[..w * h] {
            *b = 255;
        }
        for b in &mut nv12[w * h..] {
            *b = 128;
        }
        let rgb = nv12_to_rgb24(&nv12, w, h);
        for chunk in rgb.chunks_exact(3) {
            assert!(chunk.iter().all(|&c| c >= 254), "got {chunk:?}");
        }
    }

    #[test]
    fn frame_to_rgb_passthrough_for_rgb24() {
        let f = Frame {
            data: vec![10, 20, 30],
            metadata: FrameMetadata {
                width: 1,
                height: 1,
                format: PixelFormat::Rgb24,
                captured_at: Utc::now(),
                seq: 1,
            },
        };
        let rgb = frame_to_rgb24(&f).unwrap();
        assert_eq!(rgb, vec![10, 20, 30]);
    }

    #[test]
    fn frame_to_rgb_rejects_mjpeg() {
        let f = Frame {
            data: vec![0; 10],
            metadata: FrameMetadata {
                width: 1,
                height: 1,
                format: PixelFormat::Mjpeg,
                captured_at: Utc::now(),
                seq: 1,
            },
        };
        assert!(frame_to_rgb24(&f).is_err());
    }

    #[test]
    fn backend_constructs_with_defaults() {
        let b = CpuInferenceBackend::new("model.onnx");
        assert_eq!(b.model_path, "model.onnx");
        assert!((b.confidence_threshold - 0.45).abs() < 1e-6);
        assert!((b.iou_threshold - 0.45).abs() < 1e-6);
        assert_eq!(b.labels.len(), 80);
        assert_eq!(b.labels[0], "person");
        assert!(b.session.is_none());
    }

    /// Init with a nonexistent model must fail cleanly (no panic).
    #[tokio::test]
    async fn init_missing_model_fails() {
        let mut be = CpuInferenceBackend::new("/nonexistent/nope.onnx");
        let res = be.init().await;
        assert!(res.is_err());
        assert!(be.session.is_none());
    }

    /// health_check before init → ModelNotLoaded.
    #[tokio::test]
    async fn health_check_before_init() {
        let be = CpuInferenceBackend::new("x.onnx");
        let res = be.health_check().await;
        assert!(res.is_err());
    }
}
