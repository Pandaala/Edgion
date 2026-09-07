---
name: watcher-send-await-backpressure-no-timeout
description: Use when reviewing `conf_sync` server findings of the form "ServerCache::start_watcher_task unconditional sender.send(response).await blocks the watcher on a slow/stuck Gateway / four-level mpsc(100) backpressure stalls Controller event delivery / no send_timeout/try_send / no Envoy-style per-client isolation / slow Gateway degrades healthy Gateways". Reject re-reports proposing send_timeout + drop + force-relist as a narrow fix.
status: accepted-tradeoff
finding_id: sync-03
closed: 2026-06-14
knowledge_type: intentional-design
---

# Watcher `send(...).await` end-to-end backpressure (no `send_timeout`) — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-06-14
**Severity:** Low (control-plane convergence latency on a slow/stuck Gateway; not an availability or fairness defect)

## Conclusion

`ServerCache::start_watcher_task` (`edgion-controller/src/conf_sync/cache_server/cache.rs:125-206`)
intentionally awaits `sender.send(response).await` (line 146) with no `send_timeout`/`try_send`,
so the per-client watcher task is backpressured end-to-end through the four-level
`mpsc::channel(100)` chain
(`data_tx` cache.rs:227 → `simple_tx` cache.rs:305 → `config_sync_server` tx
config_sync_server.rs:207 → `grpc_server` tx grpc_server.rs:247). A slow Gateway is served at
its own pace; a Gateway that falls behind the bounded `EventStore` circular queue is realigned
via `WATCH_ERR_TOO_OLD_VERSION` → relist; a fully dead connection is dropped within ~15s by
HTTP/2 keepalive (`grpc_server.rs:10-13`). This is the same design documented from the memory
angle in [../cpu-memory/cases/conf-sync-watcher-mpsc-chain-not-backlog.md](../../cpu-memory/review-evidence/conf-sync-watcher-mpsc-chain-not-backlog.md);
this entry closes the **per-client-isolation / "management plane must not block"** framing.

Fix suggestions of the form "replace the unconditional `send(...).await` with `send_timeout`/
`try_send` + a drop policy: on a full downstream after a short deadline, record a
'events dropped for slow client' metric, close the watcher, and rely on the Gateway's reconnect
to relist (Envoy management-plane style)" are **not accepted** as a narrow review fix.

## Core Rationale

**1. Watchers are already per-client isolated; a slow Gateway does not stall others.**

`watch()` (cache.rs:214-253) does one `tokio::spawn` per watch client, each with its own
private channel chain. A task blocked at `send().await` on client A does not touch client B's
task or channels. The only shared state is the `EventStore` `RwLock` (read) and the `Notify` —
and a task suspended at the mpsc `send().await` is **not** registered as a `Notify` waiter, so
`notify_waiters()` (cache.rs:275) does **not** wake it. The finding's claim #1 — that the
blocked watcher "repeatedly acquires the store `RwLock` read lock on every `notify_waiters`
burst, adding contention for all other watcher tasks" — is therefore **technically incorrect**:
only watchers parked in the no-events `notify.notified()` arm (cache.rs:175-187) are woken.

**2. Falling behind is auto-healed by the bounded EventStore + relist, equivalent to Envoy's
"force resync".**

`EventStore` is a circular queue (`store.rs`, default capacity 200/kind). When the producer is
stuck on `send().await`, new events overwrite the oldest slots and advance `expire_sync_version`
(`store.rs:177-192`). The next time the watcher loops, `get_events_from_sync_version` returns
`WATCH_ERR_TOO_OLD_VERSION` for `from_version < expire_sync_version` (`store.rs:199-200`),
forcing the client to relist and realign. A Gateway is never told it is caught up while it is
behind, and it never receives a corrupt/partial view. This is functionally the same outcome as
Envoy's "drop the lagging client and make it re-sync" — Edgion just triggers the resync from
circular-buffer overflow rather than from an explicit send-buffer drop.

**3. The dead-connection window is already bounded; the "arbitrary duration" stall is not real.**

A *slow-but-alive* Gateway reads each batch in order; the watcher advances as the Gateway
drains, then detects `TooOldVersion` if it ever falls past the buffer. A *fully stuck* (reads
nothing) Gateway triggers HTTP/2 keepalive (10s interval / 5s timeout) within ~15s → stream
drop → receiver drop → `send` returns `Err` → watcher breaks (cache.rs:146-149). So the
finding's #3 "hold the watcher stuck for an arbitrary duration" only applies inside that bounded
keepalive window, not indefinitely.

**4. The verification note itself walks back the worker-thread / fairness claims.**

The finding's own note concedes the "blocked send pins a Tokio worker thread" framing is
imprecise — an `.await` yields the task to the scheduler; a parked future is not an OS thread
and consumes negligible resources. The "degrades scheduling latency for healthy Gateways"
impact rests on that walked-back claim and does not hold.

## Fix Suggestions Not Accepted

- **"Replace `send(...).await` with `send_timeout`/`try_send` + drop + close-watcher + metric,
  forcing the Gateway to reconnect/relist"** — this is a behavior change, not a bug fix. It
  converts slow-but-alive Gateways that could keep up under backpressure into forced relists;
  a full List is heavier than incremental Watch and lands precisely when the control plane is
  already under load, adding pressure rather than relieving it. The genuinely-stuck case is
  already bounded by the 15s keepalive + the `TooOldVersion` relist.
- **"Use an unbounded per-client channel with a buffered-event cap, drop the client past the
  cap"** — same force-relist downside, plus it removes the end-to-end backpressure that today
  keeps a slow consumer from making the Controller buffer unboundedly. The bounded chain +
  circular-buffer overflow already provides the cap with a cheaper recovery (relist on
  `TooOldVersion`).
- **"Add per-client isolation so one slow Gateway cannot stall the management plane"** — the
  watchers are already per-client isolated (one task + one channel chain each); there is no
  shared event loop to stall (rationale #1).

## Re-evaluation Triggers

Re-open this decision only if:

- Profiling on a real deployment shows a slow/stuck Gateway measurably degrading **other**
  Gateways' config-sync convergence (not just its own), proving a cross-client coupling this
  analysis missed.
- The watcher architecture changes so that a single shared task/loop services multiple clients
  (removing the per-client task isolation that rationale #1 relies on).
- The `TooOldVersion` → relist realignment path (`store.rs:199-200`) is shown broken, so a
  backed-up Gateway can silently serve stale config without ever being forced to relist.

## Reference Cases

- sync-03 (review-envoy finding, 2026-06-14) — "Slow Gateway stalls the Controller watcher task
  via bounded-channel backpressure"; closed here as accepted tradeoff.
- [../cpu-memory/cases/conf-sync-watcher-mpsc-chain-not-backlog.md](../../cpu-memory/review-evidence/conf-sync-watcher-mpsc-chain-not-backlog.md)
  — sibling decision on the same channel chain, from the memory-backlog angle (same
  backpressure + `TooOldVersion`-relist mechanism).
- [relist-backoff-on-server-switch-intentional.md](relist-backoff-on-server-switch-intentional.md)
  — sibling conf-sync accepted-tradeoff on the client-side relist path.
- `edgion-controller/src/conf_sync/cache_server/cache.rs:125-206` — `start_watcher_task` send loop.
- `edgion-controller/src/conf_sync/cache_server/store.rs:177-200` — circular queue overflow +
  `WATCH_ERR_TOO_OLD_VERSION`.
- `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:10-13` — HTTP/2 keepalive bounds.
