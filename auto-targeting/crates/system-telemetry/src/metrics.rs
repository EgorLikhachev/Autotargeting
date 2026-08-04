//! FPS / latency recording for the Phase 1.1 minimal loop.
//!
//! [`MetricsRecorder`] accumulates per-stage latency samples (capture, infer,
//! annotate, total) and computes the sustained FPS and latency percentiles
//! over the recording window. Designed to be embedded in the soak test: one
//! recorder per run, call [`record`] for every processed frame with the
//! measured stage durations in microseconds.
//!
//! At the end of the run, [`summary`] produces a compact, JSON-serialisable
//! table suitable for the Phase 1.1 article (`docs/POC_PHASE_1_1.md`).
//!
//! ## Why custom and not `hdrhistogram`
//!
//! The minimal loop needs p50/p95 of a handful of stages plus sustained FPS.
//! That's a few hundred float samples at most over 30 min; a small sorted-slice
//! percentile is exact, dependency-free, and trivially auditable. Adding
//! `hdrhistogram` for this would be over-engineering.

use serde::Serialize;
use std::time::{Duration, Instant};

/// Identifier for a pipeline stage whose latency we record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Stage {
    Capture,
    Infer,
    Annotate,
    /// End-to-end (capture → detections ready).
    Total,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Capture => "capture",
            Stage::Infer => "infer",
            Stage::Annotate => "annotate",
            Stage::Total => "total",
        }
    }
}

/// One stage's accumulated latency samples (microseconds).
#[derive(Debug, Default, Clone)]
struct StageSamples {
    samples_us: Vec<u64>,
}

impl StageSamples {
    fn record(&mut self, us: u64) {
        self.samples_us.push(us);
    }

    /// Percentile in [0, 100]. Returns 0 if no samples.
    fn percentile(&self, pct: f32) -> f64 {
        if self.samples_us.is_empty() {
            return 0.0;
        }
        let mut sorted = self.samples_us.clone();
        sorted.sort_unstable();
        let idx = ((pct / 100.0) * (sorted.len() as f32 - 1.0)).round() as usize;
        sorted[idx.min(sorted.len() - 1)] as f64
    }

    fn count(&self) -> usize {
        self.samples_us.len()
    }

    fn mean_us(&self) -> f64 {
        if self.samples_us.is_empty() {
            return 0.0;
        }
        let sum: u64 = self.samples_us.iter().sum();
        sum as f64 / self.samples_us.len() as f64
    }

    fn max_us(&self) -> u64 {
        self.samples_us.iter().copied().max().unwrap_or(0)
    }
}

/// Per-stage summary stats, serialisable.
#[derive(Debug, Clone, Serialize)]
pub struct StageSummary {
    pub stage: &'static str,
    pub count: usize,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

/// Whole-run summary, serialisable to JSON for the article / soak log.
#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub elapsed_s: f64,
    pub frames_processed: usize,
    pub sustained_fps: f64,
    pub stages: Vec<StageSummary>,
}

/// Accumulates latency samples for the pipeline stages and computes the run
/// summary.
pub struct MetricsRecorder {
    started: Instant,
    capture: StageSamples,
    infer: StageSamples,
    annotate: StageSamples,
    total: StageSamples,
    frames_processed: usize,
}

impl Default for MetricsRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRecorder {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            capture: StageSamples::default(),
            infer: StageSamples::default(),
            annotate: StageSamples::default(),
            total: StageSamples::default(),
            frames_processed: 0,
        }
    }

    /// Record one processed frame's per-stage durations (microseconds).
    /// `total_us` is the end-to-end time for that frame. Stages may be 0 if
    /// not measured (e.g. annotate skipped).
    pub fn record(&mut self, capture_us: u64, infer_us: u64, annotate_us: u64, total_us: u64) {
        self.capture.record(capture_us);
        self.infer.record(infer_us);
        self.annotate.record(annotate_us);
        self.total.record(total_us);
        self.frames_processed += 1;
    }

    /// Convenience: record a stage's latency from a [`Duration`].
    pub fn record_stage(&mut self, stage: Stage, dur: Duration) {
        let us = dur.as_micros() as u64;
        match stage {
            Stage::Capture => self.capture.record(us),
            Stage::Infer => self.infer.record(us),
            Stage::Annotate => self.annotate.record(us),
            Stage::Total => {
                self.total.record(us);
                self.frames_processed += 1;
            }
        }
    }

    /// Frames recorded so far.
    pub fn frames_processed(&self) -> usize {
        self.frames_processed
    }

    /// Elapsed wall-clock time since the recorder was created.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Compute the final summary. Sustained FPS = frames / elapsed_seconds,
    /// using the `Total` stage's sample count (frames that completed the loop).
    pub fn summary(&self) -> RunSummary {
        let elapsed_s = self.elapsed().as_secs_f64();
        let frames = self.total.count();
        let sustained_fps = if elapsed_s > 0.0 {
            frames as f64 / elapsed_s
        } else {
            0.0
        };
        RunSummary {
            elapsed_s,
            frames_processed: frames,
            sustained_fps,
            stages: vec![
                stage_summary("capture", &self.capture),
                stage_summary("infer", &self.infer),
                stage_summary("annotate", &self.annotate),
                stage_summary("total", &self.total),
            ],
        }
    }
}

fn stage_summary(name: &'static str, s: &StageSamples) -> StageSummary {
    StageSummary {
        stage: name,
        count: s.count(),
        mean_ms: s.mean_us() / 1000.0,
        p50_ms: s.percentile(50.0) / 1000.0,
        p95_ms: s.percentile(95.0) / 1000.0,
        max_ms: s.max_us() as f64 / 1000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_recorder_summary_is_zero() {
        let r = MetricsRecorder::new();
        let s = r.summary();
        assert_eq!(s.frames_processed, 0);
        assert!(s.sustained_fps < 1e-9 || s.sustained_fps.is_nan() || s.elapsed_s >= 0.0);
        for st in &s.stages {
            assert_eq!(st.count, 0);
            assert_eq!(st.p50_ms, 0.0);
        }
    }

    #[test]
    fn record_increments_frame_count_via_total() {
        let mut r = MetricsRecorder::new();
        r.record(1000, 5000, 500, 7000);
        assert_eq!(r.frames_processed(), 1);
        r.record(1100, 5100, 600, 7100);
        assert_eq!(r.frames_processed(), 2);
    }

    #[test]
    fn record_stage_total_increments_count() {
        let mut r = MetricsRecorder::new();
        r.record_stage(Stage::Total, Duration::from_micros(8000));
        assert_eq!(r.frames_processed(), 1);
    }

    #[test]
    fn record_stage_capture_does_not_increment_frames() {
        let mut r = MetricsRecorder::new();
        r.record_stage(Stage::Capture, Duration::from_micros(1000));
        assert_eq!(r.frames_processed(), 0);
    }

    #[test]
    fn percentile_basic() {
        let mut s = StageSamples::default();
        for &v in &[100u64, 200, 300, 400, 500] {
            s.record(v);
        }
        // p50 of [100,200,300,400,500] ≈ 300
        assert!((s.percentile(50.0) - 300.0).abs() < 1.0);
        // p95 ≈ near 500
        assert!(s.percentile(95.0) >= 480.0);
        assert_eq!(s.max_us(), 500);
    }

    #[test]
    fn percentile_empty_is_zero() {
        let s = StageSamples::default();
        assert_eq!(s.percentile(50.0), 0.0);
        assert_eq!(s.percentile(95.0), 0.0);
    }

    #[test]
    fn mean_us_correct() {
        let mut s = StageSamples::default();
        s.record(1000);
        s.record(3000);
        assert!((s.mean_us() - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn summary_computes_sustained_fps_after_sleep() {
        let mut r = MetricsRecorder::new();
        // Simulate 5 frames over ~0.1s → FPS ≈ 50.
        for _ in 0..5 {
            r.record(1000, 5000, 500, 7000);
        }
        std::thread::sleep(Duration::from_millis(100));
        let s = r.summary();
        assert_eq!(s.frames_processed, 5);
        // sustained FPS = 5 / elapsed. elapsed ≥ 0.1s → fps ≤ 50.
        assert!(s.sustained_fps <= 50.0 + 1.0, "fps={}", s.sustained_fps);
        // All 4 stages present.
        assert_eq!(s.stages.len(), 4);
        let total = s.stages.iter().find(|st| st.stage == "total").unwrap();
        assert_eq!(total.count, 5);
        assert!((total.mean_ms - 7.0).abs() < 1e-6);
    }

    #[test]
    fn summary_serializes_to_json() {
        let mut r = MetricsRecorder::new();
        r.record(1000, 5000, 500, 7000);
        let s = r.summary();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"sustained_fps\""));
        assert!(json.contains("\"stage\":\"total\""));
        assert!(json.contains("\"p95_ms\""));
    }

    #[test]
    fn stage_as_str() {
        assert_eq!(Stage::Capture.as_str(), "capture");
        assert_eq!(Stage::Infer.as_str(), "infer");
        assert_eq!(Stage::Annotate.as_str(), "annotate");
        assert_eq!(Stage::Total.as_str(), "total");
    }
}
