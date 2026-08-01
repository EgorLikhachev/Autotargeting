# Safety Procedures

> **This document is mandatory reading for anyone touching the auto-targeting
> codebase.** A mistake in this system can crash a real drone.

## Golden rules

1. **Fail-safe by default.** Any module that loses its input stream (video,
   detections, FC heartbeat) MUST stop sending commands. Better to lose the
   target than to crash the drone.
2. **Never bypass the state machine.** All state transitions go through
   `StateMachine::try_transition()`. The only exception is `force_transition()`,
   which is reserved for safety overrides (ABORT) and logs a warning.
3. **PWM from Orange Pi is forbidden.** All control goes through MAVLink. No
   exceptions, no "just this once."
4. **Safety pilot has final authority.** The RC override on the FC is the last
   line of defense — when the safety pilot moves the stick, auto commands
   stop. This is configured in ArduPilot, not in our code.
5. **No untested code flies.** Anything not covered by the Flight Readiness
   Criteria (below) does not go in the air.

## Emergency procedures

### Loss of video feed
- **Trigger:** `video_loop_wdt` expires (default 100 ms).
- **Action:** State machine transitions to `TRACKING_DEGRADED`. Commander stops
  sending correction commands. Tracker continues with last known state for
  `loss_hysteresis_ms` (default 500 ms).
- **If recovery:** within hysteresis → return to `TRACKING`.
- **If no recovery:** after hysteresis → transition to `LOST`, then `RTH`
  after `max_target_age_ms` (default 2000 ms).

### Loss of FC heartbeat
- **Trigger:** `fc_heartbeat_wdt` expires (default 1000 ms).
- **Action:** State machine transitions to `ABORT`. Commander attempts to send
  `MAV_CMD_NAV_RETURN_TO_LAUNCH` (RTL) — if the FC is truly dead this won't
  work, but ArduPilot's internal failsafe will trigger RTL on heartbeat loss
  regardless.
- **Recovery:** if heartbeat returns, operator must manually reset from `ABORT`
  to `IDLE` after confirming the drone is in a safe state.

### Oscillation detected
- **Trigger:** `AntiLoopGuard` detects sign-change rate > 0.5 in yaw commands.
- **First trigger:** freeze commands for 1 second, transition to
  `TRACKING_DEGRADED`.
- **Repeated triggers** (≥ 3 in 5 s): transition to `ABORT`, trigger RTH.
- **Why:** oscillations in autonomous targeting have caused more drone crashes
  than any other single failure mode. Treat them as critical.

### Operator abort
- **Trigger:** operator issues `OperatorCommand::Abort`.
- **Action:** immediate transition to `ABORT`, send RTL command.
- **Recovery:** only via `OperatorCommand::Reset` AFTER the drone has landed
  and disarmed.

## Pre-flight checklist

Before any flight with auto-targeting enabled:

- [ ] FC firmware is latest stable ArduPilot Plane
- [ ] FC failsafe configured: heartbeat loss → RTL
- [ ] RC override tested: moving the stick cancels auto mode in < 200 ms
- [ ] Battery sufficient for RTL + 20% margin
- [ ] GPS lock acquired, HDOP < 2.0
- [ ] Home position set
- [ ] Auto-targeting service running (verify with `--health-check`)
- [ ] All watchdogs registered (check `WatchdogRegistry::snapshot()`)
- [ ] Camera feed verified (latency < 50 ms)
- [ ] Test target tracking on the ground before takeoff
- [ ] Safety pilot briefed on abort procedure
- [ ] Flight log recording enabled

## Flight Readiness Criteria

The system may NOT fly until ALL of the following are satisfied. This is a
gate — one failure blocks flight.

### A. Software quality
- A1. All unit tests pass in CI.
- A2. All SITL integration tests pass in 95% of runs (10 consecutive runs).
- A3. Code coverage on critical-path crates (`commander`, `fc-adapter`,
  `target-tracker`) > 80%.
- A4. `cargo clippy -D warnings` clean on critical-path.
- A5. `cargo audit` has no critical CVEs.

### B. Performance
- B1. End-to-end latency < 150 ms on real hardware (HITL-T2).
- B2. Video latency < 50 ms.
- B3. Inference latency < 60 ms.
- B4. Lock acquisition time < 1 second.

### C. Stability
- C1. 8-hour HITL-T2 run without crash.
- C2. Watchdog triggers < 1 per hour in normal mode.
- C3. Memory growth < 50 MB over 8 hours.

### D. Safety
- D1. Every watchdog artificially triggered and recovery confirmed.
- D2. Oscillation detector tested with synthetic oscillation pattern.
- D3. RC override response < 200 ms.
- D4. RTH activation < 1 second from ABORT.
- D5. Disarm command stops motors < 500 ms.

### E. Documentation
- E1. All CRITICAL hypotheses in `HYPOTHESES.md` confirmed or mitigated.
- E2. This document (SAFETY.md) reviewed by all team members.
- E3. Flight test plan written and reviewed.
- E4. Pre-flight checklist (above) printed and on hand.

### F. Hardware
- F1. SpeedyBee F405 flashed with latest stable ArduPilot Plane.
- F2. Orange Pi 5 configured: `autotarget` user, SSH access, systemd units
  installed and autostart.
- F3. Arducam UC-852 tested on vibration rig (simulates flight vibration).
- F4. Battery and BEC verified to supply stable power to OPi + FC under full
  load (NPU + video).

## Incident response

If something goes wrong in flight:

1. Safety pilot takes over (RC override).
2. Land immediately.
3. Do NOT power off the Orange Pi — preserve logs.
4. Pull logs: `journalctl -u auto-targeting.service > incident.log`
5. File an incident report in `docs/incidents/YYYY-MM-DD-<short-desc>.md`.
6. Do NOT fly again until root cause is identified and fixed.
