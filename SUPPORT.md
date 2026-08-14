# Support

## Where to get help

| Need | Best place |
|---|---|
| **Bug or crash** | [Open an issue](https://github.com/EgorLikhachev/Autotargeting/issues/new/choose) (use the **Bug report** template) |
| **Feature request** | [Open an issue](https://github.com/EgorLikhachev/Autotargeting/issues/new/choose) (use the **Feature request** template) |
| **Question / "how do I…?"** | [GitHub Discussions](https://github.com/EgorLikhachev/Autotargeting/discussions) |
| **Security issue** | See [`SECURITY.md`](SECURITY.md) — **do not** open a public issue |

Before opening an issue, please:

1. Search the existing [issues](https://github.com/EgorLikhachev/Autotargeting/issues)
   and [discussions](https://github.com/EgorLikhachev/Autotargeting/discussions)
   — your question may already be answered.
2. Check [`CHANGELOG.md`](CHANGELOG.md) to confirm you are not seeing
   already-fixed behavior.
3. Try the latest `main` if you are on a tagged release.

## Useful documentation

- **Overview & quickstart** — [`README.md`](README.md)
- **Full results report** — [`auto-targeting/docs/PROJECT_REPORT.md`](auto-targeting/docs/PROJECT_REPORT.md)
- **Hardware test numbers** — [`auto-targeting/docs/HARDWARE_TEST_RESULTS.md`](auto-targeting/docs/HARDWARE_TEST_RESULTS.md)
- **Architecture** — [`auto-targeting/docs/ARCHITECTURE.md`](auto-targeting/docs/ARCHITECTURE.md)
- **Specification** — [`auto-targeting/docs/SDD-SPEC.md`](auto-targeting/docs/SDD-SPEC.md)
- **Safety / flight readiness** — [`auto-targeting/docs/SAFETY.md`](auto-targeting/docs/SAFETY.md)
- **KPIs & roadmap** — [`auto-targeting/docs/KPI.md`](auto-targeting/docs/KPI.md),
  [`auto-targeting/docs/ROADMAP.md`](auto-targeting/docs/ROADMAP.md)

## Reporting a good bug report

Include the following so we can reproduce quickly:

- **Rust toolchain:** `rustc --version` (MSRV is 1.75).
- **Platform:** x86 dev host, or Orange Pi 5 (RK3588) with `librknnrt.so` version.
- **The exact command** you ran (from `auto-targeting/`).
- **Full error output** and, for a crash, the backtrace
  (`RUST_BACKTRACE=1 cargo run ...`).
- For hardware issues, the tail of `output/*/telemetry.jsonl` and the last
  systemd journal lines.

## Response expectations

This is a research/development-stage project maintained by a small team.
We read every issue but cannot guarantee a response time. Issues with a minimal
reproducer and a clear write-up get attention first.

## Contributing a fix

The fastest way to get a problem solved is often to fix it yourself — see
[`CONTRIBUTING.md`](CONTRIBUTING.md).
