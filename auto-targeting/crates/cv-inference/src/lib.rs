//! CV inference module — orchestrates object detection.
//!
//! Status: 🚧 Phase 0 scaffolding only.
//!
//! ## Architecture (per ADR-0001)
//!
//! Pure-Rust bindings to RKNPU2 SDK are not yet mature (see HYPOTHESES.md H-001).
//! Therefore inference runs in a separate C++ microservice (`rknn-bridge/`),
//! and this crate provides a Rust client that communicates with it over
//! a Unix domain socket or shared memory.
//!
//! ## Fallback
//!
//! If `allow_cpu_fallback = true` in config and the bridge is unavailable,
//! `CpuInferenceBackend` (ONNX Runtime) can be used. This is much slower but
//! allows development on x86 dev machines without an NPU.

pub mod backend;
pub mod nms;

pub use backend::{CpuInferenceBackend, InferenceBackend, MockInferenceBackend, RknnBridgeClient};
pub use nms::non_max_suppression;

pub use common::Detection;
