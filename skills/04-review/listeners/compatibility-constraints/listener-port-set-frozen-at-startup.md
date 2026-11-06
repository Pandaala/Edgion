---
name: listener-port-set-frozen-at-startup
description: Review startup-only listener binding, the Pingora TCP service-registration constraint, and the accepted TCP/UDP dynamic-listener feature boundary.
knowledge_type: compatibility-constraints
---

# Listener port set is frozen at startup — accepted TCP/UDP boundary

## Conclusion

Edgion configures every listener socket once, during startup
(`create_and_configure_server()` → `gateway_base.configure_listeners()`,
`edgion-gateway/src/cli/pingora.rs`). TCP-based services are registered with Pingora
before `Server::run(RunArgs)` consumes the `Server`; the Gateway retains no runtime
service-registration handle. UDP sockets instead run as independent Tokio tasks.
`GatewayHandler::partial_update()`
(`edgion-gateway/src/runtime/handler.rs`) rebuilds only routing data (PGIS + per-listener
route managers); it does not touch sockets. Adding a listener port to a live Gateway
therefore leaves the new port unbound (no proxy accepts traffic on that port) and removing a port
leaves an orphan bound socket — a port-set change requires a gateway process restart.
TLS certificate updates are exempt (ArcSwap hot path on the existing socket).

This is an accepted, documented limitation. The supported operator workflow is a
Gateway process restart or Kubernetes rolling restart for port changes. Adding
runtime bind/drain support requires a separately authorized lifecycle design.

Gateway deletion/recreation also requires a data-plane restart, even for the
same physical listener key. That is a separate
[accepted product boundary](../intentional-design/gateway-recreation-requires-restart.md),
not proof that Pingora prevents reconnecting route publication on an existing
socket. Route and certificate hot reload under an existing Gateway remains supported.

## Core Rationale

**1. The current TCP serving model lacks runtime service registration**

The locked Edgion Pingora fork
(`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`) registers services
through `Server::add_service(&mut self, ...)`, while `Server::run(mut self,
RunArgs)` owns the serving loop (`pingora-core/src/server/mod.rs`). This was
re-verified by source inspection at that revision on 2026-11-06 after the fork
was rebased onto upstream 0.9.1; `pingora-core/src/server/` is unchanged from
`0b0d8bba` (the `edgion_0.9.0_v1` revision this page was last verified against; not the immediate predecessor lock, which was `1aaa45cc`), and the `pingora-core/src/listeners/mod.rs`
change between the two revisions (L4 buffer documentation and an unbuffered
listener test) adds no post-startup registration. The existing Gateway handler
cannot simply call `add_service` after startup. This constrains the current
integration; it is not proof that a different serving design is impossible.

UDP is outside this Pingora API constraint: `add_udp_listener` binds a socket
directly and registers an `EdgionUdpProxy` Tokio task. `UdpListenerRegistry`
currently provides registration and process-wide shutdown, not per-listener
retirement and port reuse. Its startup-only port set is an accepted product
boundary, not a dependency-imposed impossibility.

On 2026-10-07 the user explicitly declined dynamic TCP/UDP listener support and
requested removal of task `gateway-api-v16-dynamic-stream-listeners`, citing
Pingora's current limitation. No UDP-only workaround or replacement TCP serving
loop is authorized. The accepted cost is that new ports remain unbound and
removed ports remain occupied until restart; this feature gap is not fixed or
disproved. Keeping startup ownership avoids adding another listener lifecycle.
The [Gateway API capability boundary](../../../08-kubernetes/01-gateway-api/01-not-implemented.md)
continues to exclude `TCPRoute` and `UDPRoute` from advertised features until
their full conformance profiles pass. The source task recorded prebound TCP/UDP
traffic success and profile timeouts on ports created after startup; those are
historical observations, not fresh verification.

**2. The medium-term mitigations are themselves feature work**

An operator-visible warning (a `warn` log or a Gateway status condition emitted when the
PGIS holds a listener key with no bound socket) needs a new detection path and either a new
metric or a status-machine change. That is a standalone task, not a review-issue-flow patch.

**3. Historical documentation changes (`listener-01-no-runtime-port-add`)**

The following changes belong to the historical listener-01 resolution, not this
task closure. This closure only revises knowledge and references and removes the
declined task after independent review and required checks.

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
  `Server` is owned by the `Server::run(RunArgs)` serving loop; no runtime handle exists.
  Blocked on Pingora API.
- "Emit a `warn`/Gateway status condition when a configured port has no bound socket" —
  legitimate but a separate medium-term feature; promote to `tasks/todo/` if prioritized,
  do not close it under the review-issue flow.
- "Drain removed Pingora listeners gracefully like Envoy" — needs a lifecycle
  interface beyond the current integration. UDP retirement is separately
  implementable in principle but is outside the accepted feature scope.

## Verification limits

The task closure inspected the current Gateway handler, listener builder, UDP
registry, feature inventory, and locked Pingora server API. No production code,
resource fields, or tests were changed. No TCP/UDP wire test or conformance
profile was rerun for this no-implementation decision; closure checks do not
establish dynamic-listener support.

## Re-evaluation Triggers

Re-evaluate if:

- Pingora exposes post-startup `Service`/listener registration (then design runtime
  rebind/drain via brainstorming, promote to `tasks/todo/`).
- The operator-visible-warning mitigation is prioritized as its own task.
- A custom serving loop replaces the consume-by-`Server::run(RunArgs)` model.
- The user explicitly authorizes TCP or UDP dynamic listener lifecycle support.

## Reference Cases

- `tasks/review-envoy/listener-01-no-runtime-port-add.md` (closed 2026-05-30, 📝 documented limitation)
- `gateway-api-v16-dynamic-stream-listeners`: implementation declined on
  2026-10-07; accepted unsupported feature, with no production fix.
- `edgion-gateway/src/runtime/handler.rs` (`partial_update`, `rebuild_listener_gateway_infos` site)
- `edgion-gateway/src/cli/pingora.rs` (`create_and_configure_server`, `run_server`/`Server::run`)
- `edgion-gateway/src/runtime/server/listener_builder.rs`
- `skills/01-architecture/02-gateway/01-startup-shutdown.md`
