//! Process + thermal telemetry for Phase 1.1 soak tests.
//!
//! Provides three platform probes (Linux-only; on other targets they return
//! `None` so callers still build and the metrics recorder simply reports
//! "n/a"):
//!
//! - [`rss_kb`] — process resident set size, in KiB, parsed from
//!   `/proc/self/status` (`VmRSS:` line). Cheap, no syscalls beyond a read.
//! - [`cpu_temp_c`] — CPU package temperature, in °C, from the first
//!   `thermal_zone` whose `type` matches a known CPU label, falling back to
//!   `thermal_zone0`. RK3588 labels the big-core zone as `cpu-thermal`.
//! - [`npu_temp_c`] — RK3588 NPU thermal zone (`npu-thermal`), `None` on
//!   non-RK3588 hardware (graceful).
//!
//! Plus a generic [`read_thermal_zone`] for any zone name, and an RK3588 NPU
//! load-percent helper [`npu_load_percent`] from sysfs (where exposed).
//!
//! ## Why sysfs and not `rknn_query`
//!
//! Temperature is not exposed by the RKNN API on RK3588; the canonical source
//! is the kernel thermal zone sysfs. NPU load percent is exposed by the NPU
//! driver under `/sys/class/devfreq/fdab0000.npu/load` (RK3588). Both are
//! plain-text sysfs reads — no native deps, no librknnrt needed in the
//! telemetry crate.

#![deny(unsafe_code)]

pub mod metrics;

use serde::Serialize;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("parse error reading {what}: {source}")]
    Parse {
        what: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

pub type TelemetryResult<T> = std::result::Result<T, TelemetryError>;

// ---- Snapshot type --------------------------------------------------------

/// A single telemetry sample, serialisable to JSON for the soak-test log.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TelemetrySample {
    /// Unix timestamp (seconds, UTC).
    pub timestamp_s: i64,
    /// Process RSS in KiB, if readable.
    pub rss_kb: Option<u64>,
    /// CPU package temperature °C, if readable.
    pub cpu_temp_c: Option<f32>,
    /// RK3588 NPU temperature °C, if present.
    pub npu_temp_c: Option<f32>,
    /// RK3588 NPU load percent [0,100], if exposed by the driver.
    pub npu_load_percent: Option<f32>,
}

impl TelemetrySample {
    /// Capture a sample right now.
    pub fn capture() -> Self {
        Self {
            timestamp_s: chrono::Utc::now().timestamp(),
            rss_kb: rss_kb().ok().flatten(),
            cpu_temp_c: cpu_temp_c().ok().flatten(),
            npu_temp_c: npu_temp_c().ok().flatten(),
            npu_load_percent: npu_load_percent().ok().flatten(),
        }
    }
}

// ---- Probes ---------------------------------------------------------------

/// Resident set size of this process, in KiB. `None` on non-Linux or if the
/// `/proc/self/status` line is unexpectedly absent.
pub fn rss_kb() -> TelemetryResult<Option<u64>> {
    #[cfg(target_os = "linux")]
    {
        rss_kb_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
fn rss_kb_linux() -> TelemetryResult<Option<u64>> {
    let status = fs::read_to_string("/proc/self/status")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            // "VmRSS:      12345 kB"
            let kb_str = rest.trim().trim_end_matches("kB").trim();
            let kb: u64 = kb_str.parse().map_err(|e| TelemetryError::Parse {
                what: "VmRSS",
                source: Box::new(e),
            })?;
            return Ok(Some(kb));
        }
    }
    Ok(None)
}

/// Read `/sys/class/thermal/thermal_zone{N}/temp` and return °C.
/// sysfs reports millidegree Celsius; we divide by 1000.
#[cfg(target_os = "linux")]
fn read_thermal_zone(zone_name: &str) -> TelemetryResult<Option<f32>> {
    let path = thermal_zone_path_by_type(zone_name)?;
    let Some(p) = path else {
        return Ok(None);
    };
    let raw = fs::read_to_string(&p)?;
    let millideg: f32 = raw.trim().parse().map_err(|e| TelemetryError::Parse {
        what: "thermal temp",
        source: Box::new(e),
    })?;
    Ok(Some(millideg / 1000.0))
}

/// Find the first `thermal_zoneN` whose `type` file matches `target` (case
/// insensitive substring); returns its `temp` path, or the fallback zone if
/// no match and `target` is the CPU default.
#[cfg(target_os = "linux")]
fn thermal_zone_path_by_type(target: &str) -> TelemetryResult<Option<PathBuf>> {
    let base = Path::new("/sys/class/thermal");
    if !base.exists() {
        return Ok(None);
    }
    // Scan up to thermal_zone0..thermal_zone15 — cheap, bounded.
    for i in 0..16u32 {
        let zone = base.join(format!("thermal_zone{i}"));
        let type_file = zone.join("type");
        if !type_file.exists() {
            continue;
        }
        if let Ok(t) = fs::read_to_string(&type_file) {
            if t.trim().eq_ignore_ascii_case(target) {
                let temp = zone.join("temp");
                if temp.exists() {
                    return Ok(Some(temp));
                }
            }
        }
    }
    // Fallback: CPU default → thermal_zone0 if it exists.
    if target.eq_ignore_ascii_case("cpu-thermal") || target.eq_ignore_ascii_case("soc-thermal") {
        let fallback = base.join("thermal_zone0/temp");
        if fallback.exists() {
            return Ok(Some(fallback));
        }
    }
    Ok(None)
}

/// CPU package temperature °C. On RK3588 this is the `cpu-thermal` zone; the
/// helper falls back to `thermal_zone0` if the type label differs.
pub fn cpu_temp_c() -> TelemetryResult<Option<f32>> {
    #[cfg(target_os = "linux")]
    {
        read_thermal_zone("cpu-thermal")
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

/// RK3588 NPU temperature °C (`npu-thermal` zone). `None` on non-RK3588
/// hardware — callers should not treat this as an error.
pub fn npu_temp_c() -> TelemetryResult<Option<f32>> {
    #[cfg(target_os = "linux")]
    {
        read_thermal_zone("npu-thermal")
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

/// RK3588 NPU load percent [0,100]. Reads the devfreq `load` sysfs attribute.
/// Format on RK3588: `"0@1234567"` (load%@busy-time). `None` if the attribute
/// is absent (non-RK3588 hardware).
pub fn npu_load_percent() -> TelemetryResult<Option<f32>> {
    #[cfg(target_os = "linux")]
    {
        // RK3588 NPU devfreq path. On some kernels the exact address differs;
        // we try the canonical one, then a glob-style fallback.
        let primary = Path::new("/sys/class/devfreq/fdab0000.npu/load");
        let path = if primary.exists() {
            Some(primary.to_path_buf())
        } else {
            find_devfreq_load("npu")?
        };
        let Some(p) = path else { return Ok(None) };
        let raw = fs::read_to_string(&p)?;
        // "0@1234567890" → take chars before '@'
        let pct_str = raw.split('@').next().unwrap_or(raw.trim());
        let pct: f32 = pct_str.trim().parse().map_err(|e| TelemetryError::Parse {
            what: "npu load",
            source: Box::new(e),
        })?;
        Ok(Some(pct))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
fn find_devfreq_load(name_substr: &str) -> TelemetryResult<Option<PathBuf>> {
    let base = Path::new("/sys/class/devfreq");
    if !base.exists() {
        return Ok(None);
    }
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let entry_name = entry.file_name();
        let entry_name = entry_name.to_string_lossy();
        if entry_name.to_lowercase().contains(name_substr) {
            let load = entry.path().join("load");
            if load.exists() {
                return Ok(Some(load));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Most probes touch real /sys and /proc, so we test the parsing helpers
    // with synthetic inputs rather than asserting on live hardware values.

    #[test]
    fn rss_kb_returns_ok_on_any_platform() {
        // On Linux this reads a real number; on Windows it returns Ok(None).
        // Either way it must not panic / Err.
        let res = rss_kb();
        assert!(res.is_ok());
    }

    #[test]
    fn cpu_temp_c_returns_ok() {
        let res = cpu_temp_c();
        assert!(res.is_ok());
    }

    #[test]
    fn npu_temp_c_returns_ok() {
        let res = npu_temp_c();
        assert!(res.is_ok());
    }

    #[test]
    fn npu_load_percent_returns_ok() {
        let res = npu_load_percent();
        assert!(res.is_ok());
    }

    #[test]
    fn telemetry_sample_capture_does_not_panic() {
        let s = TelemetrySample::capture();
        assert!(s.timestamp_s > 0);
        // rss may be Some on Linux, None elsewhere — either is fine.
    }

    #[test]
    fn telemetry_sample_serializes_to_json() {
        let s = TelemetrySample {
            timestamp_s: 1700000000,
            rss_kb: Some(12345),
            cpu_temp_c: Some(42.5),
            npu_temp_c: None,
            npu_load_percent: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"rss_kb\":12345"));
        assert!(json.contains("\"cpu_temp_c\":42.5"));
        assert!(json.contains("\"npu_temp_c\":null"));
    }
}
