---
name: integration-orchestrators-need-owned-deadlines
description: Use when reviewing shell test schedulers, child waits, or integration timeouts.
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
