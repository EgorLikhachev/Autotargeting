# ADR-0002: Tracking Algorithm — IoU-based Greedy Matching + Kalman Filter

- **Status:** Accepted
- **Date:** 2026-08-01
- **Decision makers:** TBD
- **Related hypotheses:** —

## Context

The `target-tracker` crate must maintain target state between detections
(typically 30–100 ms apart) and survive brief occlusions (up to 2 s per
our KPI). It needs to:

1. Match new detections to existing tracks (data association).
2. Predict target position when no detection is available (occlusion).
3. Acquire a "lock" — confirm that a target is being tracked stably before
   transitioning to `TRACKING` state.
4. Declare a target `LOST` after sufficient time without detections.

## Decision

Use a **greedy IoU-based matching** algorithm combined with a **2D
constant-velocity Kalman filter** for state estimation. Defer DeepSORT
(a more sophisticated tracker using deep appearance features) until
profiling shows it's needed.

### Algorithm

#### 1. Per-track Kalman filter

Each active track owns a `KalmanFilter2D` (see `crates/target-tracker/src/kalman.rs`):
- State: `[x, y, vx, vy]` (position + velocity in image pixels)
- Observation: `[x, y]` (bbox center from detection)
- Model: constant velocity (adequate for short-term prediction)

#### 2. Detection-to-track matching (greedy IoU)

For each new frame's detections:
1. For each active track, compute IoU with each detection.
2. Greedily assign the detection with highest IoU above
   `match_iou_threshold` (default 0.3) to each track.
3. Unmatched detections → candidate new tracks (but only one active track
   is supported in Phase 0 — multi-target is Phase 3 stretch goal).
4. Unmatched tracks → increment `missed_frames`, run Kalman `predict()` only.

#### 3. Lock acquisition

When the operator selects a detection (`OperatorCommand::SelectTarget`), the
tracker creates a new track with `confirmation_count = 1`. Each subsequent
matching detection increments `confirmation_count`. When
`confirmation_count >= lock_confirmation_frames` (default 3), the tracker
declares `is_locked() = true` and the commander transitions to `TRACKING`.

This prevents false locks from a single flickering detection.

#### 4. Loss detection

A track is declared `LOST` when either:
- `missed_frames > max_missed_frames` (default 60 = 2 s @ 30 FPS), OR
- `last_seen` is older than `max_target_age_ms` (default 2000 ms).

The commander then transitions `TRACKING → LOST → RTH`.

### Simplifications vs. full SORT/DeepSORT

- **No appearance features.** We match on IoU only. This works when targets
  are visually distinct and don't cross paths frequently. For dense scenes
  with similar-looking targets, we'd need DeepSORT's appearance embedding.
- **Single active track.** Phase 0 supports only one tracked target at a
  time. Multi-target tracking (managing N tracks simultaneously) is a
  Phase 3 stretch goal.
- **No track lifecycle management.** Tracks are created on operator command
  and destroyed on `LOST` or `clear()`. We don't auto-create tracks from
  unmatched detections — the operator decides what to track.

## Consequences

**Positive:**
- Simple, fast, deterministic — easy to test and reason about.
- No dependency on a deep learning model for appearance features.
- Kalman filter is well-understood and has predictable failure modes.
- Greedy IoU matching is O(N×M) for N tracks and M detections — fine for
  N=1, M<50 (typical case).

**Negative:**
- Will fail to re-identify a target after long occlusions (>2 s) if it
  moves significantly. The Kalman prediction will be wrong, and no detection
  will match the predicted position.
- Identity switches possible if two similar-looking targets cross paths.
- No multi-target support in Phase 0.

**Neutral:**
- The Kalman filter is a simplified 2D constant-velocity model with fixed
  gains (not a full covariance-matrix implementation). Adequate for our
  short-prediction-horizon use case.

## Alternatives considered

1. **DeepSORT.** Adds a CNN-based appearance embedding for re-identification.
   Rejected for Phase 0/3 — adds significant complexity (another model to
   run on NPU) and we don't need multi-target tracking yet. Can be added
   later behind the same `TargetTracker` API.

2. **BYTETrack.** Multi-object tracker that uses low-confidence detections
   for matching. Similar trade-offs to DeepSORT — overkill for single-target.

3. **KCF (Kernelized Correlation Filter).** Classic visual tracker that
   doesn't need detections at all — tracks by appearance. Considered as a
   fallback for occlusion periods, but adds complexity. Phase 3 may add it
   as a `PredictOnly` mode fallback.

4. **Full Kalman with covariance matrix.** A 4×4 covariance matrix would
   give optimal Kalman gains dynamically. Our simplified version uses fixed
   gains. The performance difference is negligible for short prediction
   horizons (≤2 s). Can be upgraded if profiling shows the simplified
   version diverges.

## Implementation notes

- `crates/target-tracker/src/kalman.rs` — `KalmanFilter2D` (simplified).
- `crates/target-tracker/src/tracker.rs` — `TargetTracker` (single-target).
- Phase 3 will add:
  - Multi-target support (`HashMap<TargetId, ActiveTrack>`).
  - Hungarian algorithm for optimal assignment (instead of greedy).
  - Optional appearance features (DeepSORT-style).
- All thresholds are configurable via `[tracker]` section of config.toml.

## Test coverage

- `kalman.rs`: 5 tests (init, predict, update, velocity convergence, bbox center).
- `tracker.rs`: 6 tests (acquire, lock, missed frames, loss, clear, non-matching).
- All tests pass. See `docs/KPI.md` for current status.
