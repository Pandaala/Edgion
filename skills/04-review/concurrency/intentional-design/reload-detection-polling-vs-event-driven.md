---
name: reload-detection-polling-vs-event-driven
description: Use when reviewing conf_sync `grpc_server.rs` watch-stream findings that the 1-second `check_interval` polling of `provider.config_sync_server()`'s `server_id` should be replaced by an event-driven push (broadcast a terminal `WATCH_ERR_SERVER_RELOAD` to all active streams before clear/regenerate, then delete the polling loop); covers why the polling backstop is an accepted tradeoff and why the fast notification is already event-driven via `EventsLost`.
status: accepted-tradeoff
finding_id: sync-04
closed: 2026-06-14
knowledge_type: intentional-design
---

# Controller-Reload Detection via 1s Polling — DESIGN-TRADEOFF

## Conclusion

The per-Watch-stream 1-second `check_interval` that polls `provider.config_sync_server()`'s
`server_id` and sends `WATCH_ERR_SERVER_RELOAD` on change
(`edgion-controller/src/conf_sync/conf_server/grpc_server.rs:258-313`, duplicated in
`watch_server_meta` at `:424-449`) is an intentional **belt-and-suspenders backstop**, not the
primary reload-notification path. The actual fast, event-driven notification already exists:
on reload the controller calls `css.clear_all()`
(`conf_mgr/conf_center/non_kube_common.rs:243-245`) → `ServerCache::clear`'s
`notify_waiters()` (`conf_sync/cache_server/cache.rs:73`) → every watcher wakes, detects the
version discontinuity, and emits `EventsLost`, which makes the client relist immediately.

Fix suggestions of the form "replace the polling with a direct protocol-level push — broadcast
a terminal `WATCH_ERR_SERVER_RELOAD` to all active Watch streams *before* clearing the
EventStore and regenerating the `server_id`, make clear+regenerate atomic w.r.t. the
notification, and delete the polling loop" are **not accepted** as a narrow review fix.

## Core Rationale

**1. The fast notification is already event-driven; the poll is only a redundant backstop**

The live reload path does not mutate the existing server — it `clear_all()`s the old
`ConfigSyncServer`, drops it (`set_config_sync_server(None)`), then constructs a brand-new
`ConfigSyncServer::new()` with a fresh `server_id` on the next loop iteration
(`non_kube_common.rs:204-246`, `kubernetes/center.rs:820`). The `clear_all()` →
`notify_waiters()` → `EventsLost` chain fires synchronously on that clear, so a connected
gateway is told to relist within an RTT. The 1s `check_interval` only fires the *additional*
`WATCH_ERR_SERVER_RELOAD` after the new instance becomes visible — a duplicate trigger for a
relist the gateway has, in the common case, already started via `EventsLost`. Removing the poll
to make `ServerReload` "event-driven" optimizes a path that is not the bottleneck.

**2. Reload is an almost-never event and 1s is in the noise**

Controller reload / relink / leader switch are rare (rolling upgrade, manual relink, crash
recovery), not steady-state operations, and the reload itself takes seconds of controller-side
work (cache rebuild, registry re-ready, `wait_registry_ready(30, ...)`). A worst-case 1s extra
detection latency on the backstop is a small fraction of that and not separately observable.
This is the same cost profile already accepted in
[`leader-handle-polling.md`](leader-handle-polling.md) (100–200ms `AtomicBool` poll kept over
`watch::channel`) and [`relist-backoff-on-server-switch-intentional.md`](relist-backoff-on-server-switch-intentional.md)
(2–5s server-switch relist backoff kept intentionally).

**3. The proposed event-driven push is wide-impact, not a narrow fix, and adds the failure
modes those siblings warn about**

There is no registry of active Watch-stream senders that the reload loop could broadcast to —
each stream task independently captures `provider` and polls. Making `ServerReload` event-driven
requires new cross-layer infrastructure (an active-stream broadcast/registry threaded between
the reload loop in `conf_center` and the per-stream tasks in `grpc_server.rs`) plus making
clear/regenerate logically atomic w.r.t. that broadcast. That is exactly the
`watch`/`Notify`-style "only retains latest value / collapses a burst / initial-value race"
surface that `leader-handle-polling` rejected — easy to introduce a missed-notification bug for
a non-problem. Wide-impact protocol/lifecycle changes belong in `tasks/todo/` via brainstorming,
not a `/fix-issue` narrow fix.

**4. The residual costs are real but minor and cold-path**

- *Double relist*: during a reload a gateway may get both `EventsLost` (fast) and a redundant
  `ServerReload` (≤1s later); both converge to the same relist against the new store. Worst case
  is one wasted relist on a rare event — wasteful, not a correctness bug.
- *Poll overhead at scale*: N gateways × K≈17 kinds run a 1s `interval`; each tick does a lock
  read + `String` clone of `server_id`. This is constant but tiny control-plane CPU, never on
  the data-plane request path.

## Fix Suggestions Not Accepted

- **"Broadcast `WATCH_ERR_SERVER_RELOAD` to all active streams before clear/regenerate and
  delete the polling loop"** — requires a new active-stream registry/broadcast spanning the
  reload loop and per-stream tasks; the fast notification it would add already exists via
  `EventsLost`. Wide-impact lifecycle change → promote to `tasks/todo/` if ever prioritized.
- **"Make `clear` + `regenerate_server_id` atomic w.r.t. notification (regenerate, push to all
  streams, then clear)"** — predicated on mutating one server in place, but the live path
  creates a *new* instance and drops the old one; there is no single mutable server to order
  around. Restructuring the reload to in-place-mutate-and-broadcast is itself the wide change.
- **"Shorten the interval (e.g. 100ms) to tighten the gap"** — the gap is on the redundant
  backstop, not the `EventsLost` fast path; shrinking it adds tick overhead × N*K for no
  user-visible win and amplifies the synchronized-relist fanout the
  `relist-backoff-on-server-switch-intentional` jitter exists to absorb.
- **"Specify and dedupe the `EventsLost` vs `ServerReload` ordering"** — both already converge
  to a relist against the new store; the residual is a rare double-relist, not a divergence
  worth new protocol state.

## Re-evaluation Triggers

Re-open this decision only if:

- Profiling under a realistic rolling-upgrade scenario shows reload reconvergence dominated by
  the 1s detection gap rather than by controller-side restart work.
- The per-kind Watch-stream architecture is replaced by a single multiplexed stream, so a
  reload-time broadcast becomes a localized change rather than cross-layer infrastructure.
- `EventsLost` stops being emitted on `clear_all()` (e.g. the reload path changes to mutate the
  server in place without a cache clear), making the poll the *only* notification path.

## Reference Cases

- Review finding: `tasks/review-envoy/sync-04-reload-detection-polling-gap.md` (closed
  2026-06-14). Verification note in that file confirmed `reset_for_relink`
  (`config_sync_server.rs:298-308`) is dead code; the live mechanism is new-instance creation.
- `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:258-313` / `:424-449` — the two
  1s `check_interval` polling loops.
- `edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs:204-246` /
  `kubernetes/center.rs:820` — reload loop: `clear_all()` then new `ConfigSyncServer::new()`.
- `edgion-controller/src/conf_sync/cache_server/cache.rs:73` — `ServerCache::clear`
  `notify_waiters()`, the event-driven `EventsLost` trigger.
- [`leader-handle-polling.md`](leader-handle-polling.md) — sibling accepted tradeoff:
  poll-over-`watch::channel` kept for a near-zero-rate control-plane event.
- [`relist-backoff-on-server-switch-intentional.md`](relist-backoff-on-server-switch-intentional.md)
  — sibling accepted tradeoff on the gateway side of the same reload signal.
