---
name: request-trace-id-lifecycle
description: Core W3C request context and phased RequestId correlation policy, including response suppression and log masking.
knowledge_type: intentional-design
---

# Core W3C context and incremental RequestId policies

## Decision and authority

The user approved this design on 2026-09-10 while replacing the attachable
TraceContext plugin. There is one RequestId plugin. RequestTraceId is the internal
request-owned data structure; it is not another configurable plugin.

Core propagation preserves valid incoming W3C context and creates context when it
is missing or invalid. Generated roots have the sampled bit unset. This does not
enable span creation/export or add a configurable trust/sampling policy. A remote
parent ID must not be reported as a gateway span ID.

## Composition contract

- Core default x-trace-id preserves a valid destination value or uses the W3C ID.
- Global RequestId entries execute before normal request validation. Gateway
  entries execute early when TLS evidence or unique listener ownership identifies
  the owner; ambiguous shared listeners defer until canonical routing knows it.
- Route entries run at their ordinary request position. Their patches are
  incremental; a rejection before them prevents their effects. BackendRef
  RequestId is unsupported and rejected on Controller and Gateway.
- Early sources/conditions must only use available parsed-header data and the
  canonical trace ID. Body/dye and late context dependencies are rejected.
- A valid established destination value wins. Default disable applies only to
  the implicit default, not to an explicitly configured custom rule of that name.
  Suppression must survive backend echo and later response plugins until an
  authorized RequestId rule adds the name again.
- Reconcile managed upstream and response headers from live request state at
  final write boundaries. Do not enqueue a stale response value during early
  execution. Do not automatically echo traceparent or tracestate.
- At most 16 managed names include suppression tombstones; active values have a
  4096-byte aggregate limit. The usual single-ID case uses a Vec serialized as a
  JSON map. Copied external values are masked in logs, not on the wire.

The first-set rule applies to correlation headers, not a new W3C wire protocol.
Removing a header at one gateway cannot prevent the next gateway from generating
its own default. No cross-gateway deletion marker is introduced.

The response coverage contract is the main HTTP proxy, including gRPC and
WebSocket initial headers. The separate EdgionHttpRedirectProxy uses its own
context and does not execute this plugin chain. Pre-header protocol/TLS failures
cannot guarantee HTTP response IDs.

## Generation and observability boundaries

Preserve the accepted [per-reference runtime generation boundary](../../plugin-runtime/intentional-design/request-plugin-reference-generation-boundary.md).
An early reference and its later ordinary pass retain the same Arc, including
empty/missing resolution results. The same resource attached at another scope is
another occurrence. No execution flag belongs on a shared PluginRuntime and no
whole-map snapshot is added.

RequestInfo owns `trace_id.w3c.trace_id` and `trace_id.correlation_ids`; the latter
is a map with log masking for copied client/header/context values. W3C trace IDs
and business IDs may repeat across requests and must not become unique storage
keys. Gateway traffic observability remains in the canonical access log without
new data-plane tracing macros.

The integration-only AccessLogStore retains its existing last-write lookup and
store_count contract for a repeated correlation key; it is not an all-request
archive. This feature does not redesign that test API. Tests needing separate
lookups supply distinct IDs; canonical log output retains individual requests.

## Provenance and verification boundary

The original D0070 finding (`enforce-trace-context-attachment-cardinality`) was a
real composition gap: resource-local cardinality did not establish request-wide
ownership. The approved replacement removes attachable W3C ownership instead of
adding singleton validation across tiers. It does not establish a generic route
override/exclude framework.

Implementation sources are `request_trace_id/`, `ctx.rs`, `plugins/http/request_id/`,
`plugins/runtime/plugin_runtime.rs`, `routes/http/proxy_http/`, and the shared
RequestId schema/Controller attachment validators.

Verification on 2026-09-11: 27 focused core/OTLP tests, 11 final RequestId/response
regressions, nine early-dispatch tests, eight schema tests, three Controller
attachment/reference tests, and six plugin CRD checks passed. The RequestId
static integration suite passed all nine cases; RequestIdDynamic passed initial,
update, restore and deletion checks on the shared Controller/Gateway topology.
Workspace all-target check and Clippy passed (existing Clippy warnings remain),
as did format, documentation, SSA, metrics, Pingora-source, data-plane tracing,
English-content and whitespace guards. Three independent reviewers reached LGTM
after protocol, response-path and documentation corrections.

The two initial manual dynamic runs had setup failures (expired reused services,
then omitted base fixtures); the complete shared topology passed on rerun. This
is not a claim of a full integration sweep, a live Kubernetes run, a live OTLP
collector test or a physical multi-gateway deployment. A three-hop state-machine unit test covers sequential initialization and
reconciliation with generated and inherited IDs; HTTP forwarding is covered by
the echo-backend integration suite.

D0069's old TraceContext documentation gap is superseded by removing its plugin
and options, not by accepting its old trust/generation/sampling wording. The new
core always generates missing context and does not expose those old controls.

Re-evaluate on header write-path additions, ownership selection changes, KeyGet
additions, exporter integration or new plugin scopes.
