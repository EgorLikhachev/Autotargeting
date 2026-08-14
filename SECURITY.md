# Security Policy

## Reporting a vulnerability

**Do NOT open a public GitHub issue for a security vulnerability.**

If you discover a security issue in the Auto-Targeting System, please report it
privately:

- **Email:** <security@example.com>
- **Or** use GitHub's private vulnerability reporting:
  the **Report a vulnerability** button on the
  [Security advisories](https://github.com/EgorLikhachev/Autotargeting/security/advisories/new)
  page.

Please include:

1. A description of the issue and its potential impact.
2. Steps to reproduce, or a proof-of-concept.
3. The affected version/commit (run `git rev-parse HEAD` in `auto-targeting/`).
4. Any suggested mitigation or fix.

We will acknowledge receipt within **72 hours** and aim to provide an initial
assessment within **7 days**. Coordinated disclosure is the default: we will
publish a patched release and a Security Advisory together with you once a fix
is ready, crediting you unless you prefer to remain anonymous.

## Scope

This policy covers the code in this repository. It does **not** cover:

- Vulnerabilities in the proprietary Rockchip `librknnrt.so` runtime — report
  those to Rockchip via the
  [rknn-toolkit2](https://github.com/airockchip/rknn-toolkit2) project.
- Issues in ArduPilot firmware itself — report to the
  [ArduPilot security team](https://ardupilot.org/dev/docs/security.html).

## Safety-critical considerations

This software is designed to **command a flying vehicle**. Security issues that
could lead to loss of vehicle control, geofence bypass, or unintended arming
are treated as **critical**.

A few notes relevant to security:

- The default flight-controller adapter is `mock`. The system cannot command a
  real vehicle until `fc.adapter` is explicitly set to `sitl-mavlink` or
  `ardupilot-mavlink` and the
  [Flight Readiness Criteria](auto-targeting/docs/SAFETY.md) are met.
- There is no direct PWM/actuator control anywhere — all actuation goes through
  MAVLink commands. Do not add direct PWM paths.
- The operator REPL and config files are intended for trusted operators on a
  private network. Do not expose the CLI or the `rknn-bridge` Unix socket to an
  untrusted network.

## Supported versions

Only the latest release (currently `v0.1.0-phase-1.1`) receives security fixes.
Pre-release/development builds on `main` and feature branches are not covered.

| Version | Supported |
|---|---|
| `v0.1.0-phase-1.1` | ✅ |
| `main` (development) | ⚠️ best-effort |

## Hardening checklist (for deployments)

- Run the service under a dedicated unprivileged user (see the
  [`systemd`](auto-targeting/deploy/systemd/) units).
- Restrict filesystem/network permissions with systemd `ProtectSystem=`,
  `PrivateDevices=`, `NoNewPrivileges=`.
- Keep `fc.adapter = mock` until you are on the bench with a tested FC link.
- Verify geofence, battery RTH/LAND thresholds, and watchdog timeouts before
  every flight — see [`auto-targeting/docs/SAFETY.md`](auto-targeting/docs/SAFETY.md).
