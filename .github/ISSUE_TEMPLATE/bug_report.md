---
name: Bug report
about: Something is not working as expected
title: "[bug] "
labels: bug, triage
assignees: ''
---

## Summary

<!-- A 1–2 sentence description of the problem. -->

## Expected behavior

<!-- What you expected to happen. -->

## Actual behavior

<!-- What actually happened. -->

## Steps to reproduce

1.
2.
3.

```bash
# The exact command you ran (from auto-targeting/):
```

## Environment

- **Platform:** [ ] x86 dev host  [ ] Orange Pi 5 (RK3588)
- **Rust:** <!-- `rustc --version` (MSRV is 1.75) -->
- **Commit/tag:** <!-- `git rev-parse --short HEAD` in auto-targeting/ -->
- **librknnrt.so version:** <!-- on-device only, e.g. 2.3.0 -->
- **Camera / FC:** <!-- e.g. Arducam OV9782 USB / SpeedyBee F405 -->

## Output / logs

```
Paste the full error output here.
For crashes, re-run with RUST_BACKTRACE=1 and include the backtrace.
```

For hardware issues, attach the tail of `output/*/telemetry.jsonl` and the last
systemd journal lines (`journalctl -u auto-targeting -n 200`).

## Severity

- [ ] Blocks my work
- [ ] Annoying but I have a workaround
- [ ] Cosmetic / minor
