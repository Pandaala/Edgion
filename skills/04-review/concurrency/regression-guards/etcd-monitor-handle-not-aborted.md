---
name: etcd-monitor-handle-not-aborted
description: EtcdLinkClient now stores its health-monitor JoinHandle and aborts it in shutdown(), so the task is deterministically terminated and cannot hold the client read lock against the shutdown write lock. Reject re-reports claiming the monitor task is spawned-and-forgotten / not joined on shutdown.
knowledge_type: regression-guards
---

# link-sys F-012 (etcd) — health-monitor `JoinHandle` is now stored and aborted on shutdown

**Status:** ✅ fixed · 2026-05-25
**Severity:** Low (BadLogic) — spawned-and-forgotten monitor task; shutdown could block on the client write lock and had no exit confirmation

## Conclusion

`EtcdLinkClient` (`edgion-common/src/link_sys_core/providers/etcd/client.rs`) now holds
`monitor_handle: parking_lot::Mutex<Option<JoinHandle<()>>>`. `start_health_monitor` stores the
handle returned by `tokio::spawn`, and `shutdown` takes the handle and calls `handle.abort()`
right after sending the cooperative `shutdown_tx` signal and **before** acquiring the `client`
write lock. The task is therefore deterministically terminated, and a monitor parked in
`mc.status().await` / `Client::connect().await` (holding the `client` read lock) is cancelled at
its next await point so the `client.write()` in `shutdown` cannot block for the full etcd call
timeout.

Re-reports of the form "the etcd health monitor is `tokio::spawn`ed and the `JoinHandle` dropped /
the monitor task is never joined or aborted on shutdown / `shutdown` can block on the write lock
for the etcd timeout / no confirmation the task exited" are **not accepted** — verify the current
`shutdown` aborts `monitor_handle` before `client.write()` before re-reporting.

## What was wrong (before the fix)

`start_health_monitor` called `tokio::spawn(...)` and dropped the returned handle; `EtcdLinkClient`
had no field to keep it. `shutdown` sent `shutdown_tx.send(true)` (cooperative) and immediately did
`self.client.write().await`. Two consequences:

1. **Write-lock contention.** The monitor holds `client.read()` across `mc.status().await`
   (line ~183) and the reconnect `Client::connect().await` runs while a writer is waiting. Tokio's
   `RwLock` is write-preferring once a writer waits, so `shutdown` could stall up to the etcd call
   timeout (a few seconds) before clearing the client.
2. **No exit confirmation.** With the handle gone, `shutdown` (and `spawn_shutdown_if_present` in
   `runtime/store.rs`, itself run inside a `tokio::spawn`) had no way to ensure the monitor had
   stopped before the surrounding runtime was torn down — risking stale log lines or a task that
   never runs its exit path.

## Why the fix is correct

The cooperative `shutdown_tx` path is kept for the common case (a task parked at the `select!`
sleep wakes on the watch change and logs its own shutdown line). `handle.abort()` is the hard
backstop: it requests cancellation at the next await point, which (a) guarantees the task is
terminated regardless of where it is in the loop and (b) releases the `client` read lock if the
task was mid `status()` / `connect()`, so the subsequent `client.write()` proceeds immediately
instead of waiting out the etcd timeout. Ordering matters — abort is issued **before** the write
lock acquisition for exactly this reason.

`abort()` without a join is sufficient because the monitor holds no unwind-critical state (no
buffered data to flush, no external resource to release beyond the `Arc<RwLock<Option<Client>>>`,
which Rust drops safely). This mirrors the sibling Webhook `health_task` path
(`providers/webhook/manager.rs`), which also aborts without joining. The Elasticsearch `bulk_task`
(`providers/elasticsearch/client.rs`) is the one provider that additionally awaits with a timeout —
because it must drain buffered docs before returning; etcd has nothing to drain. The
`parking_lot::Mutex` is held only for the brief `take()` / store, never across an await, so there
is no lock-ordering or hold-across-await hazard.

## Fix Suggestions Not Accepted

- "Await the handle (with a timeout) like the ES `bulk_task` instead of fire-and-forget abort" —
  the ES await exists to drain buffered documents; the etcd monitor has no drainable state, so an
  await only adds latency and a timeout branch for no benefit. Abort-only matches the Webhook
  `health_task` precedent.
- "Use a `tokio::sync::Mutex` for `monitor_handle` to stay async-consistent" — the handle is only
  ever `take()`/stored synchronously and never touched across an await, so a sync
  `parking_lot::Mutex` is correct and avoids an unnecessary `.await`; this matches the ES
  `bulk_task` field type.
- "Drop the cooperative `shutdown_tx` signal now that we abort" — keep it: the cooperative path
  gives the common-case task a clean, logged exit; abort is the backstop for the mid-call case.

## Re-evaluation Triggers

Re-open only if:

- `start_health_monitor` reverts to dropping the spawn handle, or `shutdown` stops aborting it
  (re-opening the spawned-and-forgotten / write-lock-contention window).
- The monitor task gains drainable state (buffered writes, an external lease to release) — then the
  abort-only policy must be upgraded to a bounded join like the ES `bulk_task`.

## Reference Cases

- link-sys-F012 etcd (review finding, `tasks/link/`, 2026-05-25)
- [link-sys-F014](../intentional-design/webhook-active-health-abort-only.md) — the sibling webhook
  `health_task` abort-only decision; explains why that task needs no cooperative watch signal
  (holds no lock across awaits, unlike this etcd monitor).
- `providers/webhook/manager.rs` `health_task` — sibling abort-only monitor handle (same policy).
- `providers/elasticsearch/client.rs` `bulk_task` — sibling that additionally awaits to drain.
- [link-sys-F011](../superseded/webhook-manager-rwlock-intentional.md) — superseded
  Webhook runtime/JoinHandle coupling decision; not a current registry constraint.
- backends-F002 — sibling "spawned task handle management on a config-sync–driven store" fix.

## Resolution references

- `link-sys-F012` — 2026-05-25.
