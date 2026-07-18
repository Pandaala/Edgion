# AGENTS.md has no workqueue discipline rule

**Status**: done (2026-07-17)
**Priority**: P2 — the rule exists only as a file comment, and that comment is violated
**Filed**: 2026-07-17, requested by the maintainer while reviewing the conf_sync ordering design

## The rule to write

Per the maintainer, the controller's processing model is:

- Every resource kind has exactly **one** workqueue and **one** worker.
- Dequeue processing is **serial** and must not wait.
- **Every** event — apply *and delete* — enqueues a key. Nothing calls `process_*` directly.
  `on_apply` and `on_delete` must stay symmetric.
- Anything needing reprocessing goes through **requeue**.
- Handler processing must contain **no API calls, timers, sleeps, or other latency**.

## Where it is (not) written

Nowhere authoritative:

- `AGENTS.md` — nothing. (`CLAUDE.md` is a symlink to it.)
- `skills/01-architecture/01-controller/06-requeue-mechanism.md` — maps cross-resource requeue
  *triggers* (GatewayRouteIndex, SecretRefManager, ...) but says nothing about queue discipline.
- `skills/04-review/handler-requeue/handler-requeue-rule.md` — covers a handler's registration duties
  (`lookup_gateway()` in `parse()` implies `on_change`/`on_delete` index maintenance), not the queue.

The single place it is stated is a file comment in `kubernetes/resource_controller.rs`:

```
//! - Go operator-style workqueue: ALL events enqueue key, worker decides update/delete
```

It carries no weight, and it is violated three lines from where it is restated — line 428's
`// Runtime phase - enqueue key for worker` sits directly above a call that processes inline.

## Known violations to name when this lands

Write the rule as the rule; do not weaken it to match the code, and do not pretend the code complies.

1. **`on_delete` bypasses the queue** — `tasks/todo/on-delete-bypasses-workqueue.md`. All three
   backends. This is the violation that caused the 2026-07-17 cache-residue incident.
2. **Status IO in the worker path** — `tasks/todo/worker-path-blocking-status-io.md`.
   `read_status_json` / `write_error_status` are file IO in FileSystem mode and **real API calls** in
   Kubernetes mode, inside the serial worker loop. Head-of-line blocking for the whole kind.
3. **One-worker-per-kind is a convention, not a guarantee** —
   `tasks/todo/initdone-double-spawn-second-worker.md`. A second `InitDone` spawns a second worker on
   the same queue; FS and etcd do not prevent a second Init.

## Sequencing

Land this **with** the `on_delete` fix, not before it. Writing a rule the codebase openly violates,
in the same change that declines to fix the violation, is worse than writing it alongside the fix —
it teaches the next reader that the rule is decorative, which is exactly how the
`kubernetes/resource_controller.rs` comment ended up being ignored.

## Related

- `docs/superpowers/specs/2026-07-17-conf-sync-event-ordering-design.md` — the incident that
  motivated the rule

## Closing note (2026-07-17)

Landed in `AGENTS.md` ("Workqueue discipline (controller conf_sync)") together
with the `on_delete` fix, per the sequencing requirement. Rule 3 gained an
explicit init-phase carve-out (`on_init_apply` predates the worker). The
known-violations list names the status-IO and double-worker tasks; violation 1
of the original list is fixed.
