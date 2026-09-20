---
name: webhook-active-health-abort-only
description: Use when reviewing the LinkSys Webhook active health-check task lifecycle; explains why the task uses abort-only termination (no cooperative watch::Receiver / select! shutdown branch) and why that is sufficient. Reject re-reports claiming the task "never exits" / leaks, or that abort is deferred up to timeout_ms while an in-flight HTTP probe completes.
knowledge_type: intentional-design
---

# Webhook active health task is abort-only (no cooperative watch signal) — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-05-25
**Severity:** Low (BadLogic — pattern inconsistency / overstated delay)
**Note (2026-07-08)**: the legacy webhook `health.rs` module this describes was removed
entirely as dead code (`spawn_active_health_check` had zero production callers — the
production webhook path had moved to `resolver.rs` → `manager.rs::ManagedOutboundClient`).
Kept for historical design-rationale context. The current owner is
`WebhookLinkClient::retire` / `Drop`, which calls
`ManagedOutboundClient::stop_health_check`; the unified runtime table invokes
retirement on replacement/removal even if a caller retains the managed client.
The abort-only principle remains applicable, not the old manager layout below.

## Conclusion

`spawn_active_health_check` (formerly `edgion-gateway`'s `link_sys/providers/webhook/health.rs:14-75`) runs an
infinite `loop { tokio::time::sleep(interval).await; ...; client.get(..).send().await; ... }` with no
cooperative exit branch. Termination is by `JoinHandle::abort()` only: `WebhookManager::upsert`
(`manager.rs:187-190`) and `WebhookManager::remove` (`manager.rs:199-203`) abort the stored
`WebhookEntry::health_task` on every replace/delete. This stays as-is — abort-only is intentional and
sufficient for this task.

Fix suggestions of the form "add a `tokio::sync::watch::Receiver<()>` shutdown signal, wrap the loop
body in `tokio::select! { _ = sleep => {..}, _ = shutdown_rx.changed() => break }`, and send on the
channel from `upsert`/`remove` before `abort()`" are **not accepted**.

The two factual premises behind the finding are also rejected:

- **"The task never exits / leaks"** — false. `abort()` is always called on replace and delete
  (the issue body itself admits "no task leak occurs"); the table-row phrasing "never exits"
  contradicts the code.
- **"abort is deferred up to `timeout_ms` while the in-flight `send().await` runs to completion"** —
  false; see rationale 1.

## Core Rationale

**1. `abort()` cancels a task suspended at `send().await` promptly — there is no `timeout_ms` delay.**

A task parked at `client.get(..).send().await` (waiting on network IO) is **suspended**, not
executing. `JoinHandle::abort()` schedules the task and, on the next poll, drops its future in place
instead of polling it; dropping the reqwest send future cancels the in-flight request. So the old
health task is gone at the next scheduler tick, not after the HTTP probe completes or `timeout_ms`
expires. `abort()` only fails to interrupt **synchronous** code running between await points — here
that is the few-microsecond URL build (`health.rs:42-45`), which is negligible. The finding's
"deferred until the next await yield point → request runs to completion" framing misreads tokio
cancellation semantics: a suspended `.await` *is* the cancellation point.

**2. The cooperative signal etcd uses exists to release a held lock — webhook holds none.**

The etcd health monitor keeps a cooperative `shutdown_rx` (`select!` branch) specifically because it
holds the `client` read lock across `status().await` / `Client::connect().await`; the cooperative
path lets `shutdown` proceed without its `client.write()` blocking for the etcd call timeout (see
[link-sys-F012](../regression-guards/etcd-monitor-handle-not-aborted.md)). The webhook active health task
holds **no lock** across any await — it only reads `runtime.resolved_target.pick_url()` and atomics
(`runtime.healthy` / `passive_failures` / `backoff_sec`) per tick. There is no writer waiting on a
lock the task could block, so the lock-release motivation for a cooperative branch does not exist
here. The only thing a `watch` signal would add is one "shutting down" log line — pure observability
nicety, not correctness.

**3. The task holds no unwind-critical state, so hard abort is safe.**

Its only cross-await state is stack locals (`consecutive_failures` / `consecutive_successes`) plus
atomics on the old `WebhookRuntime` Arc — which is being discarded by the same `upsert`/`remove` that
aborts the task. There is no buffered data to flush and no external resource to release (contrast the
Elasticsearch `bulk_task`, which awaits with a timeout to drain buffered docs). Dropping the future
mid-flight loses nothing observable. This is the same "abort (no join) is sufficient because the task
holds no unwind-critical state" reasoning F-012 applied to etcd — and F-012's own note already cites
this webhook `health_task` as the abort-only precedent.

## Fix Suggestions Not Accepted

- **"Add a `watch::Receiver` + `select!` shutdown branch and send before abort"** — for a task that
  holds no lock and has no drainable state, this only buys a single exit log line at the cost of
  threading a receiver through `spawn_active_health_check`, the `WebhookEntry` struct, and both
  `upsert`/`remove` (which must then coordinate "send then abort"). Medium cost / low risk for a
  non-existent delay; 🔴 Fix-Value.
- **"Keep the in-flight probe from being cancelled by switching to cooperative-only (drop abort)"** —
  abort is the correct backstop; removing it would re-introduce the spawned-and-forgotten window that
  F-012 just closed for the sibling etcd path.
- **"Add a logged shutdown for parity with etcd"** — the etcd log line is incidental to the
  lock-release fix, not the goal; adding a cooperative branch solely to emit it inverts cost/benefit.

## Re-evaluation Triggers

Re-open this decision only if:

- The webhook active health task starts holding a lock (or any `RwLock`/`Mutex` guard) across one of
  its `.await` points, or gains drainable buffered state — then it would need the same cooperative +
  bounded-drain treatment as the ES `bulk_task`.
- A future change makes the per-probe synchronous section long-running (CPU-bound) such that `abort()`
  could be meaningfully deferred between await points.

## Reference Cases

- link-sys-F014 (review finding, `tasks/link/`, 2026-05-25).
- [link-sys-F012](../regression-guards/etcd-monitor-handle-not-aborted.md) — sibling etcd monitor; explains
  why the cooperative `shutdown_rx` is needed there (lock release) and cites this webhook task as the
  abort-only precedent.
- [link-sys-F011](../superseded/webhook-manager-rwlock-intentional.md) — superseded decision on the
  `WebhookEntry { runtime, health_task }` coupling and why the `RwLock` registry stays.
- Formerly `edgion-gateway`'s `link_sys/providers/webhook/health.rs` (removed 2026-07-08) — `spawn_active_health_check`.
- `edgion-gateway/src/link_sys/providers/webhook/manager.rs` — `WebhookEntry`, `upsert` / `remove`
  abort sites.

## Resolution references

- `link-sys-F014` — 2026-05-25 (⚠️ accepted tradeoff).
