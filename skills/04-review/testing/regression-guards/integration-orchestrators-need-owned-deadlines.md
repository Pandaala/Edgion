---
name: integration-orchestrators-need-owned-deadlines
description: Use when reviewing shell test schedulers, child waits, or integration timeouts.
knowledge_type: regression-guards
---

# Integration orchestrators need owned deadlines

Readiness polling and per-test timeouts do not bound a complete integration job. Every scheduler
that waits for child scripts must enforce a configurable deadline around the whole child lifecycle,
including setup, Docker operations, test execution, and teardown. Aggregate runners also need a
whole-run budget so a sequence of individually bounded jobs cannot exceed the intended run window.

Use `utils/run_with_deadline.py` for these boundaries. It starts the command in a new session, sends
TERM to that owned process group at the deadline, waits a bounded grace period, then sends KILL.
Callers treat exit 124 as `TIMEOUT`, preserve the child log, and continue to their final summary.
Do not replace state-based readiness checks with sleeps or kill processes by global name.

## Process-group probes and zombie members

`ITEST-DEADLINE-EPERM` exposed a real timeout-classification defect on macOS:
after TERM, the unreaped leader was a zombie, `killpg(pgid, 0)` raised EPERM,
and the wrapper exited 1 instead of 124. A direct lifecycle experiment observed
successful probing while running, EPERM in state Z, and ESRCH after `wait()`.
The [XNU implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)
filters zombies in `killpg1` and can return EPERM when no eligible member remains.
This is not evidence that arbitrary permission failures mean a group is gone.

The shared helper now disambiguates EPERM from probes and TERM/KILL through
`group_has_live_members`: a successful `ps -axo pgid=,stat=` snapshot must show
no non-zombie member of the owned PGID before the error is suppressed. A live
member preserves the permission failure; failed, timed-out, or malformed
snapshots fail visibly. The snapshot command has a one-second timeout. Other
groups never become signal targets. The leader remains unreaped until the final
`wait()`, retaining its PID while cleanup uses it as the group identity.

The grace period still bounds escalation; an exceptional snapshot adds bounded
query overhead. A leader's exit alone is not proof that descendants exited.
Processes that deliberately leave the owned session/group are outside this
helper's ownership boundary. Environments hiding group members from `ps` are
outside the verified platform assumptions; reassess before relying on this
fallback in such an environment.

## Verification and re-evaluation

Focused regression command:
`PYTHONDONTWRITEBYTECODE=1 python3 cicd/checks/tests/run_with_deadline_test.py`.
On 2026-09-09, all seven tests passed on macOS arm64 with Python 3.9 and a Linux
arm64 container with Python 3.11 and procps: normal
and reserved child exits, timeout exit 124, TERM-resistant child, surviving
descendant after leader exit, unreaped leader, and injected permission/snapshot
failure boundaries, including an empty snapshot. The minimal Linux fixture
initially lacked `ps`; after installing procps in the disposable container the
complete suite passed. Independent macOS review also exercised real SIGTERM
and SIGINT forwarding, yielding 143 and 130 without tracebacks.
Repository checks passed: `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`
(warnings in unchanged Rust files), and the agent-doc, SSA-force,
metrics-inventory, and Pingora source-policy guards. No full integration sweep
or `cargo test --all` was run for this focused helper change.
The original ITEST-006 entry below remains historical evidence.

Re-evaluate when changing session ownership, leader reaping, signal forwarding,
process visibility, supported `ps` implementations, or timeout exit mapping.

## Resolution references

- `ITEST-006` — 2026-08-26.
- `ITEST-DEADLINE-EPERM` — 2026-09-09; corrected macOS zombie-group handling.
