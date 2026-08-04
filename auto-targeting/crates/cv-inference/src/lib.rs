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
// The rknn-bridge client uses Unix domain sockets (`std::os::unix::net`),
// which only exist on Unix targets. On non-Unix (e.g. Windows dev machines)
// the module is absent so the crate still compiles for cpu-onnx development.
#[cfg(unix)]
pub mod bridge_client;
pub mod nms;

#[cfg(feature = "cpu-onnx")]
pub mod cpu_onnx;

pub use backend::{
    InferenceBackend, MockInferenceBackend, RknnBridgeClient as RknnBridgeClientStub,
};

// When the `cpu-onnx` feature is ON, the real ONNX-backed `CpuInferenceBackend`
// (in `cpu_onnx`) shadows the stub defined in `backend`. Otherwise the stub
// remains so dependents (cli, commander) still compile on minimal builds.
#[cfg(not(feature = "cpu-onnx"))]
pub use backend::CpuInferenceBackend;
#[cfg(feature = "cpu-onnx")]
pub use cpu_onnx::CpuInferenceBackend;

#[cfg(unix)]
pub use bridge_client::{RknnBridgeClient, RknnBridgeConfig};
pub use nms::non_max_suppression;

pub use common::Detection;
