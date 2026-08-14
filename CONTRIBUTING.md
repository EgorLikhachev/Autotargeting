# Contributing to Auto-Targeting System

Thanks for your interest in contributing! This software can command a flying
vehicle, so a few extra review rules apply — please read this file before
opening a pull request.

> First time here? Start with [`auto-targeting/docs/ARCHITECTURE.md`](auto-targeting/docs/ARCHITECTURE.md)
> for the high-level design and [`auto-targeting/docs/SAFETY.md`](auto-targeting/docs/SAFETY.md)
> for the safety model.

---

## 1. Development environment

Follow the [Installation](README.md#installation) and
[Prerequisites](README.md#prerequisites) sections of the README. In short:

```bash
git clone https://github.com/EgorLikhachev/Autotargeting.git
cd Autotargeting/auto-targeting
cargo build --workspace
cargo test --workspace
```

All `cargo` commands run from inside `auto-targeting/` (the Rust workspace).
The toolchain is pinned via `rust-toolchain.toml` (stable, MSRV 1.75).

To build the C++ NPU microservice (Unix only):

```bash
cd rknn-bridge
cmake -B build -DBUILD_TESTS=ON
cmake --build build -j
```

---

## 2. Branching model

We use a lightweight **GitHub Flow**:

- Branch off `main`. Keep `main` always green and deployable.
- One focused change per branch.
- Open a PR against `main`. Squash-merge on approval.

### Branch naming

```
<type>/<short-scope>-<topic>
```

Examples: `feat/tracker-kalman`, `fix/v4l2-deadlock`, `docs/readme-badges`,
`perf/capture-drop-old`, `refactor/bridge-protocol`.

---

## 3. Commit messages — Conventional Commits

We follow [Conventional Commits](https://www.conventionalcommits.org/) so the
changelog and version bumps can be generated automatically.

```
<type>(<scope>): <imperative summary up to ~72 chars>

<optional body: what and why, wrapped at 100 cols>

<optional footer: Closes #123, Refutes H-7, ADR-0003>
```

### Types

| Type | When to use |
|---|---|
| `feat` | New user-facing capability |
| `fix` | Bug fix |
| `perf` | Performance improvement (no behavior change) |
| `refactor` | Code restructuring (no behavior change) |
| `docs` | Documentation only |
| `test` | Test additions/improvements |
| `chore` | Build, CI, deps, tooling |
| `diag` | Temporary diagnostic/diagnostic-prints (should be short-lived) |

### Scopes (crates/modules)

`common`, `video-capture`, `cv-inference`, `yolov8`, `cv-visualizer`,
`system-telemetry`, `target-tracker`, `fc-adapter`, `commander`, `cli`,
`rknn`, `rknn-bridge`, `hw`, `sdd`, `ci`, `deps`.

### Examples

```
feat(tracker): KalmanFilter2D prediction step + tests
fix(rknn): apply sigmoid to class scores (RKNN export emits raw logits)
perf(video-capture): drop-old capture policy (try_send vs blocking_send)
docs(hw): Phase 1.1 closed — end-to-end detections via C++ bridge
chore(ci): move workflows to repo root + working-directory: auto-targeting
```

> **Diagnostic commits:** when debugging on hardware, prefixed `diag:` commits
> are acceptable to preserve the investigation trail, but they should be
> cleaned up (reverted or folded) before merging to `main`.

---

## 4. Before you submit — checks must pass

Run from `auto-targeting/`:

```bash
cargo fmt --check                                    # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lint (zero warnings)
cargo test --workspace                               # unit tests
cargo deny check                                     # license / advisories
```

CI runs all of the above on every PR — they must be green. The
[Nightly](https://github.com/EgorLikhachev/Autotargeting/actions/workflows/nightly.yml)
workflow additionally runs the full SITL integration suite.

---

## 5. Pull request process

1. Open the PR against `main`.
2. Fill in the [PR template](.github/PULL_REQUEST_TEMPLATE.md) — summary, type
   of change, the safety checklist (if applicable), and related issues/ADRs.
3. Make sure CI is green. Rebase onto the latest `main` if needed.
4. Request review. At least one approval is required for `main`.
5. Squash-merge. The squash-commit message should follow Conventional Commits.

### Review checklist (what reviewers look for)

- [ ] `cargo clippy` / `cargo fmt` clean, tests pass
- [ ] No new `unsafe` without an ADR justifying it
- [ ] No direct PWM / actuator control (must go through MAVLink)
- [ ] New config keys documented in [`auto-targeting/config.example.toml`](auto-targeting/config.example.toml)
- [ ] KPI changes reflected in [`auto-targeting/docs/KPI.md`](auto-targeting/docs/KPI.md)
- [ ] Hypotheses confirmed/refuted updated in [`auto-targeting/docs/HYPOTHESES.md`](auto-targeting/docs/HYPOTHESES.md)
- [ ] CHANGELOG entry added under **Unreleased** for user-facing changes

### Safety-critical changes — extra rules

Anything touching `commander/`, `fc-adapter/`, `target-tracker/`, watchdog
timeouts, the state machine, or anti-loop logic:

- Requires **two** approvals.
- State-machine transition changes → update the `state_machine` tests.
- Watchdog/timeout changes → document in an ADR.
- Anti-loop logic changes → oscillation tests must still pass.

See [`auto-targeting/docs/SAFETY.md`](auto-targeting/docs/SAFETY.md).

---

## 6. Reporting issues

Use the issue templates (`.github/ISSUE_TEMPLATE/`):

- **Bug report** — include Rust toolchain version, target platform (x86 dev
  vs Orange Pi 5), relevant `cargo` command, and the full error output.
- **Feature request** — describe the use case and which roadmap phase it
  advances.

For a crash on hardware, attach the telemetry tail
(`output/*/telemetry.jsonl`) and the last lines of the systemd journal.

---

## 7. Code style

- Formatting: `rustfmt` with the project's [`rustfmt.toml`](auto-targeting/rustfmt.toml)
  (4-space indent, 100-col width, Unix LF). Run `cargo fmt` before committing.
- Linting: `clippy` with [`auto-targeting/clippy.toml`](auto-targeting/clippy.toml)
  (`msrv = 1.75`, `cognitive-complexity-threshold = 25`).
- Editor config: [`.editorconfig`](.editorconfig) keeps indentation/line
  endings consistent across editors.
- Dependencies: all additions go through `cargo deny check` (license +
  advisory policy via [`auto-targeting/deny.toml`](auto-targeting/deny.toml)).

Write code that reads like the surrounding code: match its comment density,
naming, and idioms. Prefer the existing abstractions (e.g. the
`InferenceBackend` / `VideoSource` / `FlightControllerAdapter` traits) over
new ones.

---

## 8. Decision records (ADR)

Non-trivial architectural decisions are recorded as ADRs in
[`auto-targeting/docs/ADR/`](auto-targeting/docs/ADR/) and summarized in
[`auto-targeting/docs/sdd/decisions.md`](auto-targeting/docs/sdd/decisions.md).
If your change introduces or reverses a decision, add/append an ADR.

Thanks for helping make Auto-Targeting safer and better! 🛩️
