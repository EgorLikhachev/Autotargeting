## Summary

<What does this PR change? Why?>

## Type of change

- [ ] Bug fix (non-breaking change which fixes an issue)
- [ ] New feature (non-breaking change which adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to not work as expected)
- [ ] Documentation update
- [ ] Refactor / cleanup
- [ ] Test addition / improvement

## Phase

Which Roadmap phase does this advance? (e.g. "Phase 1: Video Capture")

## Safety checklist

For changes affecting the auto-targeting pipeline (anything in `commander/`,
`fc-adapter/`, `target-tracker/`, or safety-critical configuration):

- [ ] All unit tests pass (`cargo test --workspace`)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `cargo fmt --check` is clean
- [ ] If state machine transitions changed: `state_machine` tests updated
- [ ] If watchdog timeouts changed: documented in ADR
- [ ] If anti-loop logic changed: oscillation tests still pass
- [ ] If a new hypothesis is introduced: added to `docs/HYPOTHESES.md`
- [ ] No new `unsafe` code without justification in ADR
- [ ] No direct PWM control added (must go through MAVLink)

## Testing

- [ ] Unit tests added / updated
- [ ] Integration tests run against SITL (if applicable)
- [ ] Manual smoke test: `cargo run -p auto-targeting-cli -- --mock-all`

## KPIs affected

Does this change any KPIs in `docs/KPI.md`? If yes, update the table.

## Hypotheses

Does this PR confirm or refute any hypothesis in `docs/HYPOTHESES.md`? If yes,
update its status and test result.

## Related issues

Closes #NNN
Refutes/Confirms H-NNN
Implements ADR-NNNN
