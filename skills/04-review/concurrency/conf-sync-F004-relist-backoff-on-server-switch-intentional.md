---
name: conf-sync-F004-relist-backoff-on-server-switch-intentional
description: Use when reviewing `gateway/conf_sync/cache_client/event_dispatch.rs::ClientCache::start_watch` "2-5s backoff+jitter still applied after intentional server-id-change break / asymmetry vs grpc_client.rs's start_watch_server_meta" findings. Reject re-reports that propose adding a `server_switched` flag to skip the outer-loop sleep on the server-id-change path.
---

# `ClientCache::start_watch` keeps backoff+jitter on server-id-change relist — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-05-28
**Severity:** Low (BadLogic — control-plane convergence latency on controller failover)

## Conclusion

`ClientCache::start_watch`
(`src/core/gateway/conf_sync/cache_client/event_dispatch.rs:135-228, 285-296`) intentionally
keeps the existing outer-loop backoff (`BACKOFF_INIT_SECS = 2s` + `0..3s` jitter, lines 149-160)
on every non-first iteration, including the iteration that follows `break 'watch_block` at
line 295 when a changed `server_id` is observed mid-stream. This stays as-is.

Fix suggestions of the form "introduce a `server_switched` boolean (analogous to
`conf_client/grpc_client.rs:153,178,202-204`), set it on the server-id-change break, and skip
the sleep in the outer loop to mirror `start_watch_server_meta`" are **not accepted**.

## Core Rationale

**1. The `start_watch` path has a 17×-fanout × N-gateway thundering-herd profile that
`start_watch_server_meta` does not.**

`ClientCache::start_watch` is instantiated once per watched resource kind, so each gateway
runs 17 concurrent outer loops sharing the same controller endpoint. When the controller's
`server_id` rotates (rolling upgrade / leader election / crash recovery), every kind on every
gateway receives the new id within the same RTT and would all `break 'watch_block` in
near-lockstep. With the sleep removed, the relist storm against the freshly-started controller
is `17 × N_gateway` parallel `list` RPCs landing inside the same millisecond window — exactly
the failure mode the `0..3s` jitter was added to absorb. `start_watch_server_meta`
(`conf_client/grpc_client.rs:121-218`) has no such fanout: it is a single
`WatchServerMeta` stream per gateway, so its `server_switched` short-circuit only collapses
`N_gateway` reconnects, not `17 × N_gateway`. The asymmetry between the two paths is the
correct response to the different fanout, not a bug.

**2. The base 2s backoff is bounded and self-correcting; the jitter is the load-bearing
component.**

After a successful list, `backoff_secs` is reset to `BACKOFF_INIT_SECS = 2` (line 182), so the
worst-case extra delay on a server-switch relist is `2s + 0..3s jitter`. During this window the
gateway serves traffic from the in-memory cache snapshot built before the switch — it does not
drop or misroute requests; it only delays picking up any control-plane changes the operator
made during the controller restart. For a deployment whose controller restart itself takes
seconds, the extra 2-5s of staleness is within the same order of magnitude as the restart
itself and not separately observable.

**3. Removing only the base 2s while keeping jitter is also not justified at the issue's cost
profile.**

A more surgical variant — "set `server_switched`, in the backoff block sleep only the jitter
component, skip the `backoff_secs` base" — would save an average ~2s per controller restart
while preserving thundering-herd protection. It is rejected here because: (a) controller
restarts are infrequent (rolling upgrades, leader election, crashes — not steady-state
operations); (b) saving ~2s on a control-plane reconvergence that already takes seconds of
controller-side startup work is not a measurable user-facing win; (c) the cost is added state
(`server_switched` flag, one-shot consumption discipline) that every future maintainer of this
already-dense reconnect loop must keep in mind. Net new mental load for a non-measurable win.

**4. `start_watch_server_meta`'s `continue` is not a precedent that should be propagated.**

The `conf_client/grpc_client.rs:202-204` `continue` predates the multi-kind `ClientCache`
architecture and reflects the simpler "one stream per gateway" topology of the meta channel.
It is correct for that channel — but copying it across to the 17-kind generic path conflates
two different fanout regimes. The asymmetry should be read as "two different problems, two
different solutions", not as "the kind-watcher forgot to do what the meta-watcher does".

## Fix Suggestions Not Accepted

- **"Add `let mut server_switched = false;` outside the outer loop, set it on the line-295
  break, gate the `tokio::time::sleep` on `!server_switched`, and reset after consumption"** —
  removes the thundering-herd jitter that protects the freshly-started controller from a
  synchronized `17 × N_gateway` relist burst; trades a non-measurable convergence-latency win
  for a real availability risk on the controller side.
- **"Set `server_switched` and in the backoff block sleep only the jitter, skip the
  `backoff_secs` base"** — saves ~2s per controller restart on an event that takes seconds of
  controller-side work anyway; adds permanent state-tracking complexity to a dense reconnect
  loop for an unmeasurable win. Consider only if profiling shows the 2s baseline is a
  material fraction of end-to-end reconvergence on a real deployment.
- **"Mirror `start_watch_server_meta`'s `continue` verbatim"** — the two paths have different
  fanout profiles (1-per-gateway vs. 17-per-gateway); the meta path's choice is correct for
  the meta path and wrong for the kind-watcher path.

## Re-evaluation Triggers

Re-open this decision only if:

- Profiling under a realistic rolling-upgrade scenario shows controller-switch reconvergence
  dominated by the 2-5s outer-loop sleep rather than by controller-side startup work — i.e.,
  the sleep is the bottleneck, not a small fraction of total convergence time.
- The kind-watcher architecture changes so that `ClientCache::start_watch` no longer
  multiplies per kind (e.g., a single multiplexed watch stream replaces the 17 independent
  loops), eliminating the 17×-fanout thundering-herd concern.
- The controller adds an explicit "I am ready, you may relist now" admission signal that
  itself spreads load (e.g., a controller-issued backoff hint), making client-side jitter
  redundant.

## Reference Cases

- conf-sync-F004 (review finding, `tasks/conf/conf-sync-F004-relist-backoff-on-server-switch.md`,
  2026-05-28).
- [conf-sync-F002-partial-update-under-lock-intentional.md](conf-sync-F002-partial-update-under-lock-intentional.md)
  — sibling conf-sync accepted-tradeoff: a control-plane cost intentionally kept because the
  fix's structural complexity is out of proportion to the cold-path saving.
- `src/core/gateway/conf_sync/cache_client/event_dispatch.rs:135-228, 285-296` —
  `ClientCache::start_watch` outer backoff loop and server-id-change break.
- `src/core/gateway/conf_sync/conf_client/grpc_client.rs:121-218` —
  `start_watch_server_meta`, the asymmetric `server_switched`/`continue` reference cited in
  the issue.
