---
name: leader-jobs-call-site-consolidation
description: Use when reviewing controller leader-only service start/stop call sites in center.rs; covers the fixed dead-stop deletion, the single-list helpers, and three rejected hardening proposals (trait registry, catch_unwind+abort, is_leader recheck).
status: fixed
finding_id: leader-job-call-site-cleanup
closed: 2026-07-16
knowledge_type: regression-guards
---

# Leader-Job Call Sites — FIXED (helpers), three hardening proposals REJECTED

## Conclusion

The three leader-only services (ACME, conf_sync CA rotation, control-plane DNS resolver)
are started/stopped through exactly two associated fns on `KubernetesCenter` —
`start_leader_jobs(client)` / `stop_leader_jobs()` in
`edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs` — the single source of
truth for the leader-only job list. The two formerly hand-copied stop triples in
`run_leader_only_lifecycle` (the `MainFlowExit::Shutdown` and `LostLeadership` arms) were
**dead code** (unconditionally preceded by the stop inside `run_main_flow`'s cleanup) and
were deleted.

Fix suggestions of the form "add a `LeaderJob` trait registry", "wrap each start/stop in
`catch_unwind` + `abort()`", or "re-check `is_leader()` before the caches-ready start"
are **not accepted**.

## Core Rationale

**1. The helpers deliberately carry no log line**

The INFO lines stay at the all-serve call sites (`start_leader_services` /
`stop_leader_services`); the leader-only main-flow stop site stays silent. That stop site
also runs on the caches-timeout path where nothing was ever started, so a log inside the
helper would be false more often than true. Keep the helpers observability-neutral.

**2. The start/stop asymmetry around `requeue_all` is correct, not a bug**

`start_leader_services` (all-serve) calls `requeue_all().await` after `start_leader_jobs`;
the leader-only path does not. All-serve runs the controllers on followers too (status
writes gated on leadership in `resource_controller.rs`), so a newly promoted replica has
warm caches and needs a forced reconcile; leader-only starts controllers only after
winning and `runtime.reset()` clears the registry on every exit. Documented on
`start_leader_services`.

**3. `start_conf_sync_rotation` has a side effect beyond "start a worker"**

It sets the module-global `LEADER` flag (`conf_sync_ca/rotation.rs`) that gates the Admin
API's manual-rotation endpoint (`decide_trigger` / `trigger_manual_rotation`). Do not
reorder or conditionally skip it inside `start_leader_jobs`.

## Fix Suggestions Not Accepted

- **`LeaderJob` trait + `static JOBS: &[&dyn LeaderJob]` registry** — runtime polymorphism
  over a statically-known three-element set in one module; its unit test would need a fake
  `kube::Client` and `#[tokio::test]` just to assert a `for` loop iterates. The two
  helpers give the same one-line-per-new-job property at a tenth the size.
- **`catch_unwind` → `std::process::abort()` around each start/stop** — the guarded code
  has no reachable panic (parking_lot lock, atomic add, sized mpsc channel, `tokio::spawn`,
  tracing macro, two `Arc` allocations); the real panic surface lives inside the spawned
  workers, unreachable from the caller's `catch_unwind`; and `abort()` destroys its own
  diagnostic — the file layer is `tracing_appender::non_blocking` and the shipped
  deployment sets `console: false`, so SIGABRT loses the queued line (would not reproduce
  in dev, where console defaults to `true`).
- **`is_leader()` re-check before the caches-ready start** — the flag is 2–10 s stale by
  design (election loop tick + renew-deadline grace), and the guard's failure mode is
  worse than the one it fixes: on a stale `false` the jobs are skipped but `CachesReady`
  still fires, so the replica reports ready and serves conf_sync with all three leader
  jobs dead. Any stale start is torn down by the main-flow cleanup within a few hundred
  ms. The leadership race belongs to the leader-election event-loop work
  (`tasks/todo/leader-wedge-recovery-and-worker-death.md`), not a call-site guard.

## Re-evaluation Triggers

Re-open this decision only if:

- The leader-only job set grows enough (≈8+) or gains per-job config/ordering constraints
  that a declarative registry starts paying for itself.
- A start/stop path gains genuinely fallible logic (I/O, parsing) — then per-job error
  handling, not `catch_unwind`, is the tool.
- The leader-election event loop is reworked so `CachesReady` and job start can be gated
  atomically on leadership (see the wedge-recovery task).

## Reference Cases

- Original task: `tasks/todo/leader-job-call-site-cleanup.md` (2026-07-16, closed with the fix)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs` — `start_leader_jobs` /
  `stop_leader_jobs` and their four call sites
- Black-box regression: `edgion-tests` `AcmeTestSuite` (leader-only start path),
  `DnsResolutionTestSuite` (DNS resolver); conf_sync CA rotation has no integration suite

## Resolution references

- `leader-job-call-site-cleanup` — 2026-07-16.
