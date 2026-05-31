---
name: listener-port-set-frozen-at-startup
description: Use when reviewing findings that compare Edgion against Envoy LDS and claim "listener ports cannot be added/removed at runtime / partial_update never re-binds sockets / new ports are silently dropped without restart". Covers why runtime listener (re)binding is a Pingora-API-blocked feature, not a narrow review fix, and what was documented instead.
---

# Listener port set is frozen at startup; runtime rebinding is a Pingora-blocked feature — KNOWN-LIMITATION

## Conclusion

Edgion binds every listener socket exactly once, during startup Phase 1
(`create_and_configure_server()` → `gateway_base.configure_listeners()`,
`src/core/gateway/cli/pingora.rs`). The Pingora `Server` is then consumed by
`run_forever()`, so no runtime code path holds a handle to `bind()`/`listen()` a new
socket or drain an old one. `GatewayHandler::partial_update()`
(`src/core/gateway/runtime/handler.rs`) rebuilds only routing data (PGIS + per-listener
route managers); it does not touch sockets. Adding a listener port to a live Gateway
therefore leaves the new port unbound (traffic silently dropped) and removing a port
leaves an orphan bound socket — a port-set change requires a gateway process restart.
TLS certificate updates are exempt (ArcSwap hot path on the existing socket).

This is an accepted, now-documented limitation, **not** a narrow review fix. Runtime
listener add/remove/drain (Envoy LDS parity) depends on Pingora's `Server` API gaining
the ability to register `Service`s after `run_forever()` starts; until then the
documented operator workflow is a Kubernetes rolling restart for port changes.

## Core Rationale

**1. The real fix is blocked by an upstream API limit**

Zero-downtime runtime listener binding is exactly what Envoy LDS does, but it requires the
serving framework to expose post-startup socket registration/draining. Pingora's `Server`
takes ownership in `run_forever()` and offers no such hook today. Edgion cannot synthesize
this on its own; the capability is upstream-gated, so it cannot be "fixed" by patching
`partial_update`.

**2. The medium-term mitigations are themselves feature work**

An operator-visible warning (a `warn` log or a Gateway status condition emitted when the
PGIS holds a listener key with no bound socket) needs a new detection path and either a new
metric or a status-machine change. That is a standalone task, not a review-issue-flow patch.

**3. What was done instead (the in-scope work)**

- Documented the limitation in
  `skills/01-architecture/02-gateway/01-startup-shutdown.md` ("Known limitation: listener
  port set is frozen at startup").
- Added a `KNOWN LIMITATION` comment at the `rebuild_listener_gateway_infos` call in
  `partial_update`.
- Clarified the previously misleading log line — it now reads
  `"Gateway updated (routes/TLS hot-reloaded; listener port add/remove requires a process restart)"`
  instead of the old `"dynamic listener/TLS config updated"`, which implied port changes
  took effect.

## Fix Suggestions Not Accepted

- "Make `partial_update` call `server.add_service()` for new ports" — not possible: the
  `Server` is owned by `run_forever()`; no runtime handle exists. Blocked on Pingora API.
- "Emit a `warn`/Gateway status condition when a configured port has no bound socket" —
  legitimate but a separate medium-term feature; promote to `tasks/todo/` if prioritized,
  do not close it under the review-issue flow.
- "Drain removed listeners gracefully like Envoy" — same upstream-API dependency as the add
  path.

## Re-evaluation Triggers

Re-open only if:

- Pingora exposes post-`run_forever()` `Service`/listener registration (then design runtime
  rebind/drain via brainstorming, promote to `tasks/todo/`).
- The operator-visible-warning mitigation is prioritized as its own task.
- A custom serving loop replaces the consume-by-`run_forever()` model.

## Reference Cases

- `tasks/review-envoy/listener-01-no-runtime-port-add.md` (closed 2026-05-30, 📝 documented limitation)
- `src/core/gateway/runtime/handler.rs` (`partial_update`, `rebuild_listener_gateway_infos` site)
- `src/core/gateway/cli/pingora.rs` (`create_and_configure_server`, `run_server`/`run_forever`)
- `src/core/gateway/runtime/server/listener_builder.rs`
- `skills/01-architecture/02-gateway/01-startup-shutdown.md`
