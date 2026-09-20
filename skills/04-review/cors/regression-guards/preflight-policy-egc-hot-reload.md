---
name: preflight-policy-egc-hot-reload
description: Use when changing global preflight policy publication, listener construction, or request-scoped CORS preflight handling.
status: fixed
finding_id: B0065
knowledge_type: regression-guards
---

# Preflight Policy Follows EGC Hot Reload at the Request Boundary

## Conclusion and scope

`EdgionGatewayConfig.spec.preflightPolicy` is resolved into the immutable HTTP
request configuration snapshot. Each new `EdgionHttpContext` copies the mode and
status together with the request-body and forwarded-identity policies from one
snapshot lookup. The listener's `PreflightHandler` is stateless.

The request-filter preflight gate must use the same request-pinned policy for
both detection and the no-CORS fallback response. Native Gateway API CORS and
the normal EdgionPlugins CORS pipeline retain sole ownership of configured CORS
requests.

## Rationale and evidence

EGC updates publish a new `ArcSwap` snapshot without rebuilding an already
serving listener. A handler that clones `preflightPolicy` during listener
construction therefore keeps stale behavior indefinitely. Resolving the policy
in `EdgionGatewayConfigSnapshot` and copying its compact representation in
`pg_new_ctx::new_ctx` makes updates visible to new requests while preserving one
configuration generation for an in-flight request.

The request path performs the existing single snapshot load and bounded map
lookup. It adds no lock, parsing, or allocation for preflight policy. Missing or
deleted configurations use `cors-standard` and status 204. An invalid status is
kept unchanged so `PreflightHandler` rejects it before trace reconciliation or a
final-response claim, preserving the existing 500 fallback.

## Alternatives and limits

Rebuilding listeners on every EGC update is not required and would couple a
small request policy to socket lifecycle. Reading the snapshot independently in
preflight detection and response handling is also rejected because one request
could observe two publication generations.

`AllOptions` still delegates a route with CORS configuration even when the
request lacks `Origin`; only the no-CORS global fallback returns the configured
status. This guard does not change native CORS, EdgionPlugins conditions,
preflight continuation, trace reconciliation, final-response ownership, or
access-log behavior.

## Verification and re-evaluation

Gateway unit coverage exercises initial publication, update, old-request
pinning, deletion and default lookup, stateless handler modes and CORS
delegation, the no-CORS fallback status, and invalid-status fail-close behavior.

Re-evaluate if EGC updates begin rebuilding listeners, HTTP request policies
move to another publication boundary, or CORS ownership moves out of the native
and normal route-plugin pipelines.

## Resolution references

- B0065 — preflight policy hot-reload correction.
- `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs`
- `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs`
