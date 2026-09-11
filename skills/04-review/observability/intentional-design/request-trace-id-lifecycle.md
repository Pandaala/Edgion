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

On 2026-09-11 the user approved a flat access-log ID map and reserved
`x-trace-id` for the canonical W3C identity. This supersedes the initial nested
log shape and the initial preservation of conflicting inbound `x-trace-id`.

Core propagation preserves valid incoming W3C context and creates context when it
is missing or invalid. Generated roots have the sampled bit unset. This does not
enable span creation/export or add a configurable trust/sampling policy. A remote
parent ID must not be reported as a gateway span ID.

## Composition contract

- Core default x-trace-id always uses the W3C ID, replacing conflicting incoming
  values. Custom destinations named x-trace-id are rejected case-insensitively.
- Core W3C initialization is unconditional. Configured global and Gateway
  RequestId entries run early only after unambiguous logical Gateway ownership
  and scoped policy admission. Ambiguous shared listeners defer them until
  canonical routing; TLS handshake evidence alone does not authorize a class.
- Route entries run at their ordinary request position. Their patches are
  incremental; a rejection before them prevents their effects. BackendRef
  RequestId is unsupported and rejected on Controller and Gateway.
- Early sources/conditions must only use available parsed-header data and the
  canonical trace ID. Body/dye and late context dependencies are rejected.
- A valid established custom destination value wins. Default disable removes
  the reserved wire header but retains W3C identity in the access log. Suppression
  must survive backend echo and later response plugins; only a default-policy
  patch can re-enable the reserved header.
- Reconcile managed upstream and response headers from live request state at
  final write boundaries. Do not enqueue a stale response value during early
  execution. Do not automatically echo traceparent or tracestate.
- At most 16 managed names include suppression tombstones; active values have a
  4096-byte aggregate limit. The usual single-ID case uses a Vec serialized as a
  JSON map. Copied custom values follow the log masking policy without altering
  wire values. The reserved key is emitted once from W3C state before the custom
  entries, even when its header is disabled; it is not treated as a copied ID.

The first-set rule applies to correlation headers, not a new W3C wire protocol.
Removing a header at one gateway cannot prevent the next gateway from generating
its own default. No cross-gateway deletion marker is introduced.

The response coverage contract is the main HTTP proxy, including gRPC and
WebSocket initial headers. The separate EdgionHttpRedirectProxy uses its own
context and does not execute this plugin chain. Pre-header protocol/TLS failures
cannot guarantee HTTP response IDs.

## Generation and observability boundaries

The 2026-09-11 plugin-policy migration supersedes the earlier
[per-reference generation decision](../../plugin-runtime/superseded/request-plugin-reference-generation-boundary.md)
with a [captured class scope](../../plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).
Early and later passes use the same scope and retain selected runtime Arcs,
including missing results. The same resource attached at another tier is another
occurrence. Execution flags stay request-owned, never on shared PluginRuntime.
A later route owned by a different Gateway cannot reuse already-executed early
policy; it fails closed before executing that other Gateway's plugins.

RequestInfo owns a flat `trace_id` map: `trace_id["x-trace-id"]` is the W3C ID,
and other names are active custom IDs with log masking for copied
client/header/context values. Header sources observe current managed headers;
no original incoming x-trace-id snapshot is retained. W3C trace IDs
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

Historical verification of the initial nested implementation on 2026-09-11
(before the scoped-policy migration): 27 focused core/OTLP tests, 11 final RequestId/response
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

Flat-map follow-up verification on 2026-09-11: 28 core tests, 12 RequestId/
response regressions, eight schema tests and 92 test-client library tests passed.
The shared topology passed RequestId (9), RequestIdDynamic, WebhookKeyGet (18),
RequestMirror (11) and HeaderModifier (9). Transport reported 74 successful
entries: 62 executed cases plus 12 existing explicit protocol-inapplicable
skips. Workspace check/Clippy, format, repository guards and the OTLP shutdown
script regression passed. Two independent reviewers reached LGTM after test
consumer corrections. The initial runner stopped on unavailable macOS loopback
aliases; LBRoundRobin was not run. The remaining suites used the existing shared
start/stop scripts with 127.0.0.1 fixtures. A missing Filters/base Gateway was
added to the test invocation before the successful rerun. No full sweep, live
Kubernetes/OTLP collector, physical multi-gateway or cache-residue coverage is
claimed. See `tasks/pending/request-trace-id/flat-access-log.md` for scope.

D0069's old TraceContext documentation gap is superseded by removing its plugin
and options, not by accepting its old trust/generation/sampling wording. The new
core always generates missing context and does not expose those old controls.

Re-evaluate on header write-path additions, ownership selection changes, KeyGet
additions, exporter integration or new plugin scopes.

The `KeyGet::TraceId` addition was re-evaluated on 2026-09-11. It remains a local read of the
canonical request-owned W3C identity and is intentionally distinct from reading the complete
`traceparent` header or the suppressible `x-trace-id` projection. The Kubernetes condition-schema
alignment is guarded under
[`resources/regression-guards/edgion-plugins-crd-type-enum-drift.md`](../../resources/regression-guards/edgion-plugins-crd-type-enum-drift.md).
