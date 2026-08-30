---
name: request-body-policy-egc-hot-reload
description: Use when reviewing whether EdgionGatewayConfig request-body policy is frozen at listener construction or requires Gateway requeue/rebind to update.
---

# BF-01 — Request-Body Policy Uses an Atomic EGC Snapshot

## Conclusion

`EdgionGatewayConfig.spec.requestBody` is parsed on the gateway configuration
update path and published with the EGC resource in one immutable
`EdgionGatewayConfigSnapshot`. Each new HTTP request loads the snapshot, looks
up the EGC named by the listener's GatewayClass, and copies the compact
`ResolvedRequestBodyConfig` into its context.

An EGC update therefore changes the body capture switch, memory spill
threshold, physical body cap, and storage-operation timeout for subsequent
requests without restarting the process or rebinding the listener. Deletion
and missing references use the built-in default policy.

## Core Rationale

### Configuration work stays off the request path

Human-readable byte sizes are parsed only when `full_set` or `partial_update`
builds the next snapshot. The request path performs one lock-free `ArcSwap`
load, one bounded `HashMap` lookup, and a `Copy`; it does not parse strings or
take a blocking lock.

### Resource and derived policy share one epoch

The EGC resource map and its resolved body-policy map are fields of the same
snapshot and are published by one `ArcSwap::store`. Two independent stores
would permit a request to observe a new resource paired with the previous
derived policy.

### The scope remains one GatewayClass per process

This fix does not add multi-GatewayClass support. Edgion's supported deployment
model remains one Controller/Gateway instance per GatewayClass. The lookup is
keyed by the referenced EGC name so the listener can distinguish an explicit
reference from a GatewayClass with no `parametersRef`, for which built-in
defaults remain stable.

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

- Edgion formally supports multiple GatewayClasses in one Gateway process.
- A request-body policy becomes listener- or route-specific rather than
  GatewayClass-level.
- GatewayClass `parametersRef` changes are required to retarget already-bound
  listeners dynamically.

## Reference Cases

- `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs`
- `skills/04-review/concurrency/listener-gateway-info-dual-arcswap-merged-snapshot.md`
