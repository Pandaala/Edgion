---
name: request-body-policy-egc-hot-reload
description: Use when reviewing whether EdgionGatewayConfig request-body policy is frozen at listener construction or requires Gateway requeue/rebind to update.
knowledge_type: mandatory-design
---

# BF-01 — Request-Body Policy Uses an Atomic EGC Snapshot

## Conclusion

`EdgionGatewayConfig.spec.requestBody` is parsed on the gateway configuration
update path into an immutable `ResolvedGatewayConfig`. Listener construction
binds the EGC named by its GatewayClass through `bind_gateway_config`. Each new
HTTP request loads one generation from that stable `GatewayConfigHandle` and
copies its compact `ResolvedRequestBodyConfig` into the request context.

An EGC update therefore changes the body capture switch, memory spill
threshold, physical body cap, and storage-operation timeout for subsequent
requests without restarting the process or rebinding the listener. Deletion
and missing references use the built-in default policy.

## Core Rationale

### Configuration work stays off the request path

Human-readable byte sizes are parsed only when `full_set` or `partial_update`
builds the next snapshot. The request path performs one lock-free `ArcSwap`
load and a `Copy`; it does not look up the resource table, parse strings or
take a blocking lock.

### One immutable runtime generation per EGC

`publish_snapshot` refreshes each live binding with a complete resolved runtime
generation under the configuration writer lock. A request pins that generation,
including its body policy. Atomicity is per EGC, not across EGC resources or
between the runtime binding and the raw-resource/admin view. Deletion installs
defaults into the existing handle; recreation refreshes that same handle.

### Each class retains its own EGC binding

The `kubernetes-gateway-class-allow-list` expansion supports multiple classes
matching one Controller identity. `runtime/server/base.rs` resolves each Gateway's
class and parametersRef before binding its listeners. The existing per-EGC handle
keeps distinct classes' policies separate; a class without parametersRef keeps
built-in defaults. The former single-class premise is superseded. Deliberate
cross-class sharing of one physical listener port remains unsupported.

## Fix Suggestions Not Accepted

- Rebuild or rebind Pingora listeners on every EGC update. Socket lifetime is
  unrelated to a request-scoped body policy and rebinding introduces avoidable
  disruption.
- Requeue Gateway resources in the Controller. The data plane receives EGC
  updates directly through conf_sync; Controller requeue would not update the
  live proxy object.
- Parse request-body sizes or read the mutable EGC resource store per request.
  Both violate the resolved-before-hot-path invariant.
- Expand the fix into multi-GatewayClass shared-port support. That changes the
  documented product model and requires a separate design decision.

## Re-evaluation Triggers

- Cross-class physical listener sharing is introduced.
- A request-body policy becomes listener- or route-specific rather than
  GatewayClass-level.
- GatewayClass `parametersRef` changes are required to retarget already-bound
  listeners dynamically.

## Reference Cases

- `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs`
- `skills/04-review/concurrency/regression-guards/listener-gateway-info-dual-arcswap-merged-snapshot.md`

## Resolution references

- `BF-01` — 2026-07-28.
