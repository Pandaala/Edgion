---
name: relist-backoff-on-server-switch-intentional
description: Use when reviewing `gateway/conf_sync/cache_client/event_dispatch.rs::ClientCache::start_watch` "2-5s backoff+jitter still applied after intentional server-id-change break / WATCH_ERR_SERVER_RELOAD / WATCH_ERR_VERSION_UNEXPECTED / WATCH_ERR_TOO_OLD_VERSION / WATCH_ERR_NOT_READY / WATCH_ERR_SERVER_ID_MISMATCH / stream-ended / watch-setup-failed exits / asymmetry vs grpc_client.rs's start_watch_server_meta" findings. Reject re-reports that propose adding an `is_signal_break` / `skip_backoff` / `server_switched` flag to skip the outer-loop sleep on any of the four `break 'watch_block` paths.
---

# `ClientCache::start_watch` keeps backoff+jitter on every watch-break relist — DESIGN-TRADEOFF

**Status:** ⚠️ accepted tradeoff · 2026-05-28 (server-id branch) · 2026-05-28 (extended to all four break paths)
**Severity:** Low (BadLogic — control-plane convergence latency on controller failover / reload / version-skew)

## Conclusion

`ClientCache::start_watch`
(`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:135-381`) intentionally keeps the
existing outer-loop backoff (`BACKOFF_INIT_SECS = 2s` + `0..3s` jitter, lines 149-160) on every
non-first iteration, regardless of which `break 'watch_block` exit triggered the relist. After
a successful list, `backoff_secs` is reset to 2s (line 182), so the loop does **not** escalate
permanently — every signal-driven relist incurs exactly the same flat 2s + 0–3s jitter. This
stays as-is across all four exit points:

1. **Watch-setup failure** (line 258-262) — `client.watch(request).await` returns `Err`.
2. **Stream ended** (line 361-363) — `stream.message()` returns `Ok(None)`.
3. **gRPC status errors** (line 365-376) — `WATCH_ERR_VERSION_UNEXPECTED`,
   `WATCH_ERR_TOO_OLD_VERSION`, `WATCH_ERR_NOT_READY`, `WATCH_ERR_SERVER_ID_MISMATCH`.
4. **Server-side protocol signals** (line 272-282, 287-295) — `watch_response.err` non-empty
   (including `WATCH_ERR_SERVER_RELOAD`) or `watch_response.server_id` change.

Cases 3 and 4 are "controller is reachable and healthy, it deliberately told the client to
relist"; the sleep on these paths is the cost paid to preserve thundering-herd protection
across the larger fanout. Cases 1 and 2 are genuine transport failures where the sleep is
straightforwardly correct.

Fix suggestions of the form "introduce an `is_signal_break` / `skip_backoff` / `server_switched`
boolean (analogous to `conf_client/grpc_client.rs:153,178,202-204`), set it on any of the
signal-driven breaks (cases 3 and 4), and skip the sleep in the outer loop to mirror
`start_watch_server_meta`" are **not accepted** — neither for the single-branch (F004) framing
nor for the four-branch (F015) framing.

## Core Rationale

**1. The `start_watch` path has a 17×-fanout × N-gateway thundering-herd profile that
`start_watch_server_meta` does not.**

`ClientCache::start_watch` is instantiated once per watched resource kind, so each gateway
runs 17 concurrent outer loops sharing the same controller endpoint. When the controller's
`server_id` rotates (rolling upgrade / leader election / crash recovery) or the controller
emits a `WATCH_ERR_SERVER_RELOAD` / version-skew signal, every kind on every gateway receives
it within the same RTT and would all `break 'watch_block` in near-lockstep. With the sleep
removed, the relist storm against the freshly-started controller is `17 × N_gateway` parallel
`list` RPCs landing inside the same millisecond window — exactly the failure mode the `0..3s`
jitter was added to absorb. Broadening the short-circuit from the single server-id-change
branch (F004) to all four signal-driven breaks (F015) does not reduce the fanout — it makes
the synchronized relist even more likely, because `WATCH_ERR_SERVER_RELOAD` is a deliberately
broadcast signal that hits every kind on every gateway at once. `start_watch_server_meta`
(`conf_client/grpc_client.rs:121-218`) has no such fanout: it is a single `WatchServerMeta`
stream per gateway, so its `server_switched` short-circuit only collapses `N_gateway`
reconnects, not `17 × N_gateway`. The asymmetry between the two paths is the correct response
to the different fanout, not a bug.

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
- **"Add `let mut is_signal_break = false;` (or `skip_backoff`), set it before every `break
  'watch_block` that corresponds to a healthy-controller signal — `WATCH_ERR_SERVER_RELOAD`,
  `WATCH_ERR_VERSION_UNEXPECTED`, `WATCH_ERR_TOO_OLD_VERSION`, `WATCH_ERR_NOT_READY`,
  `WATCH_ERR_SERVER_ID_MISMATCH`, and the `server_id`-change branch — and check it in the outer
  loop to skip the sleep"** (F015 framing) — same rejection. `WATCH_ERR_SERVER_RELOAD` in
  particular is a broadcast signal that arrives on every kind simultaneously; gating the
  sleep on it produces the worst-case `17 × N_gateway` synchronized relist exactly when the
  controller is mid-reload and least equipped to absorb the burst. Broadening F004's single
  branch to four branches strictly enlarges the thundering-herd surface, not the win.
- **"Set `server_switched` / `is_signal_break` and in the backoff block sleep only the jitter,
  skip the `backoff_secs` base"** — saves ~2s per controller restart on an event that takes
  seconds of controller-side work anyway; adds permanent state-tracking complexity to a dense
  reconnect loop for an unmeasurable win. Consider only if profiling shows the 2s baseline is
  a material fraction of end-to-end reconvergence on a real deployment.
- **"Replace the 2s sleep with a short fixed delay (e.g. 100ms) for signal-triggered relists,
  keeping exponential backoff only for genuine transport failures"** (F015 alternative
  framing) — same fanout objection: 100ms × `17 × N_gateway` is still a synchronized burst
  inside the controller's own reload window, just at a slightly later moment. The base 2s
  exists to space the reconnect attempts beyond the controller's startup tail, not to throttle
  steady-state traffic.
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
  2026-05-28) — single-branch (server_id change at line 295) framing.
- conf-sync-F015 (review finding,
  `tasks/conf/conf-sync-F015-backoff-not-reset-after-watch-error.md`, 2026-05-28) —
  four-branch framing covering watch-setup-failed / stream-ended / gRPC-status-errors /
  server-protocol-signals (`WATCH_ERR_SERVER_RELOAD`). Note: F015's description initially
  framed this as "backoff escalates permanently"; the file was self-corrected to "flat 2s
  delay on signal-driven relists" because `backoff_secs` is reset to 2s after every
  successful list (line 182). The corrected framing is the one this entry rejects.
- [partial-update-under-lock-intentional.md](partial-update-under-lock-intentional.md)
  — sibling conf-sync accepted-tradeoff: a control-plane cost intentionally kept because the
  fix's structural complexity is out of proportion to the cold-path saving.
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:135-381` —
  `ClientCache::start_watch` outer backoff loop and all four `break 'watch_block` exits.
- `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs:121-218` —
  `start_watch_server_meta`, the asymmetric `server_switched`/`continue` reference cited in
  both F004 and F015.
