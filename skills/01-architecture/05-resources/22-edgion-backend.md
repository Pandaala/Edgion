---
name: resource-edgion-backend
description: EdgionBackend AI/A2A/MCP variants, Controller resolution, route dependencies, conf_sync transport, and unified Gateway snapshots.
---

# EdgionBackend resource

> Common flow: [00-resource-flow.md](00-resource-flow.md)
> External schema and complete examples: [../../02-features/03-resources/22-edgion-backend.md](../../02-features/03-resources/22-edgion-backend.md)

`EdgionBackend` is a namespaced, typed `HTTPRoute.backendRef` with an exactly-one variant union:
`spec.ai` describes an AI provider, `spec.a2a` describes an A2A transport target, and `spec.mcp`
describes a stateless Streamable HTTP upstream. Configuring zero or multiple variants is invalid.
`AiProxy` consumes only AI and ordinary forwarding consumes A2A. The Controller classifies a direct
HTTPRoute as MCP-eligible when its sole rule satisfies the strict exact-path contract; no separate
McpGateway resource or placement marker is used. This eligibility stage does not by itself activate
client-facing MCP traffic.

## Controller processing

`EdgionBackendHandler` follows the normal serialized workqueue flow:

1. `preparse` runs `EdgionBackendSpec::validate`. Structural errors become
   `Accepted=False/Invalid`.
2. `parse` clears the previous Secret, Service, and cross-Namespace dependency edges plus all
   Controller-resolved fields before processing the selected variant. This full replacement is
   required for delete-create coalescing and cross-variant hot replacement.
3. For AI, each credential is independently authorized through `ReferenceGrant` and resolved
   through `AiApiKeyResolver`. Resolution is partial-good; zero usable credentials deactivates the
   backend.
4. For A2A external targets, parse validates and publishes a normalized HTTP(S) endpoint. For A2A
   Service targets, it registers the Service dependency, authorizes a cross-Namespace reference,
   and proves that the named Service and numeric port exist. Missing, denied, or missing-port
   targets leave `resolvedA2a` absent so the Gateway cannot activate stale transport state.
5. For MCP, parse registers the Service and optional bearer Secret before resolving either one.
   It publishes `resolvedMcp` only when the complete target and credential are valid. The fixed
   Secret key is `bearerToken`; the shared bearer resolver enforces RFC 6750 header-safe material.
6. `on_delete` clears all variant dependency edges and requeues typed dependents through their own
   workqueues.

An EdgionBackend apply or delete requeues referencing HTTPRoutes and
EdgionBackendTrafficPolicies through the shared typed backend dependency index. Service and
ReferenceGrant changes requeue A2A Service backends through their respective dependency indices.
HTTPRoute reconcile registers terminal EdgionBackend references after delegation flattening; EBTP
reconcile registers typed policy targets. No handler calls another handler directly.

## Variant transport boundaries

Each `credentialPool.credentials[]` entry flattens the shared `SecretSlot`:

- `secretRef` is operator-facing and structural in the CRD;
- resolved `secret` is `#[schemars(skip)]`, remains serde-visible for conf_sync, and uses the
  shared Admin redaction adapter;
- `name` is a stable, non-secret alias used for status and bounded access-log attribution;
- Controller and Gateway logs never contain resource bodies, credential values, or provider URLs.

The Gateway generic config-client endpoint is `/configclient/edgionbackend`. Under the Admin
serialization guard every resolved Secret is rendered as `[redacted]`; conf_sync still carries the
real value over its trusted channel.

For A2A, operator-visible `spec.a2a` contains only `target` and `allowInsecureHttp`.
`spec.resolvedA2a` is `#[schemars(skip)]` but remains serde-visible so conf_sync can carry the
Controller-proved Service identity or normalized external endpoint. It is recomputed on every
generation and is never accepted from the CRD. WP4 adds no credentials, Agent Card retrieval,
request-body observation, JSON-RPC observation, or response observability.

For MCP, `resolvedMcp` carries either the Controller-proved Service identity plus absolute MCP path
or the normalized external HTTP(S) endpoint. The optional resolved bearer Secret follows the same
schemars-skip, conf_sync-preservation, and Admin-redaction contract as other `SecretSlot` users.
MCP resolution is fail-close and all-or-nothing rather than AI's partial-good pool behavior.

## Status

- `Accepted=False/Invalid` reports structural validation errors.
- For AI, `ResolvedRefs=True` means every credential reference resolved successfully.
- `PartiallyInvalid=True/InvalidCredentials` reports a bounded count plus credential aliases and
  fixed reason enums for omitted credentials. It can coexist with `ResolvedRefs=False` while the
  remaining usable credentials keep the service active.
- For AI, `ResolvedRefs=False` means one or more credential references failed. The Gateway deactivates the
  service only when no credential remains usable.
- For A2A, valid static configuration produces `Accepted=True`. `ResolvedRefs=False` uses
  `RefNotPermitted` or `TargetNotFound` for a denied/missing Service or port; external targets have
  no object reference and resolve during parse. Invalid exactly-one, target-union, endpoint, or
  Service-reference configuration produces `Accepted=False/Invalid` and
  `ResolvedRefs=False/Invalid`.
- For MCP, `ResolvedRefs=False` distinguishes a denied/missing target or Secret from malformed
  bearer material without putting dependency details or Secret values in status. `Accepted=False`
  remains reserved for the strict schema and static protocol/target contract.

Route `ResolvedRefs` is intentionally separate from the EdgionBackend's own status. An existing and
authorized backendRef is resolved at the route layer even when its selected variant cannot publish
an activatable runtime. Requests then fail closed with a generic response until a later
Controller-to-Gateway generation becomes usable.

## Gateway runtime

`EdgionBackendStore` is the sole writer and publishes one immutable ArcSwap snapshot containing
`RuntimeEdgionBackend::{Ai,A2a,Mcp}`. The existing `AiBackendStore` name is only a compatibility type
alias to that same store. Variant switches, invalidations, deletes, and recreates replace one entry
atomically; no request can observe both variants from one generation. Cold-path compilation:

- revalidates the spec;
- decomposes the endpoint into dial domain, port, TLS/SNI, Host authority, and path prefix;
- retains only usable credentials, their weights, optional RPM/TPM limits, and stable aliases;
- builds a collision-free canonical-model and alias table (at most 256 models and 32 aliases per
  model);
- parses optional prices into fixed-point integers;
- retains `defaults.streaming` and optional `defaults.maxOutputTokens`.
- for A2A, accepts only Controller-proved `resolvedA2a`, retains the stable logical EdgionBackend
  identity, and compiles either a typed Service/port or the external endpoint's dial address,
  TLS/SNI, HTTP authority, and confined path prefix.
- for MCP, requires exact `resolvedMcp` proof, compiles the Service/path or shared external endpoint
  transport, parses the optional bearer through the canonical resolver again, and exposes only a
  redacted header-injection value.

Equivalent updates reuse stable runtime and backend-policy handles. Invalid, unresolved, or
zero-credential updates remove the previous activatable generation rather than serving stale
state. Each request captures an immutable generation, so hot reload cannot mix variants or target
generations across attempts.

The snapshot starts at generation zero and advances exactly once for each semantic runtime-map
change. Empty, invalid-absent, and equivalent updates retain the exact snapshot Arc; changed target,
UID, or bearer material replaces it. A standalone snapshot-bound `SelectedMcpBackend` proves the
selected logical slot, direct policy identity, runtime UID, transport, credential seam, and
generation. Later WP10 runtime stages consume that proof only after the HTTPRoute exact-shape and
current-variant gates; WP9 and the Controller eligibility stage do not activate HTTP dispatch.

`defaults.timeout` does not exist. `HTTPRoute.timeouts` owns request and per-attempt deadlines;
EdgionBackendTrafficPolicy `connection.connectTimeout` owns connection establishment.

## Route and attempt consumption

An ordinary HTTPRoute may mix A2A EdgionBackend slots with ordinary Service slots. It uses the
canonical WP2 logical-backend selection, retry, timeout, session-persistence, accounting, and
access-log lifecycle. Missing, denied, invalid, or wrong-variant slots retain normal Gateway API
invalid-share behavior; selection never invents a second resolver, dialer, TLS builder, retry
scheduler, circuit breaker, or accounting path.

An A2A Service target uses the referenced Service only for EndpointSlice discovery. The direct
EdgionBackend identity owns EBTP and BackendTLSPolicy attachment; nested Service policies are not
composed into it. It reuses the canonical Service selector and session-affinity builder; A2A
scoped views are bounded to 1024 endpoints and remain request-local so retired backend UID
generations cannot recreate process-global state, while ordinary Service affinity remains cached. It
keeping the direct EdgionBackend and nested Service identities in the affinity scope. An A2A
external target uses the canonical routed-external `EgressResolver`, captured gateway egress
policy, canonical TLS peer construction, and the normalized endpoint authority/path prefix.

The variant-aware EBTP capability matrix is documented in
[20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md).
BackendTLSPolicy targets the direct `edgion.io/EdgionBackend`: it supplies the canonical upstream
TLS validation, CA/mTLS, SNI, and certificate identity for both A2A target shapes. A TLS policy on a
cleartext external endpoint fails closed. For A2A Service plus `upstreamAuthority`, the policy's
rendered Host/SNI must remain compatible with the BackendTLSPolicy hostname.

WP4 is transport wiring only. Agent Card retrieval and JSON-RPC gateway behavior belong to WP6;
bounded request-body observation belongs to WP5; response outcome and SSE task observability
belong to WP8.

### AI attempt consumption

The compiled HTTPRoute rule supplies every candidate as a typed EdgionBackend backendRef.
AiProxy reads the canonical OpenAI request once, resolves a request-stable logical model, applies
quota, and stores a bounded attempt plan. It does not store backend names or mutate an upstream
jump slot.

A cross-Namespace backendRef without a matching ReferenceGrant retains its declared weight as an
invalid Gateway API slot, but it is never materialized as an AI candidate. The dispatch boundary
rechecks the same `ref_denied` decision by backend slot before any provider request is prepared.

For each upstream attempt the Gateway:

1. selects an eligible route backend with `backendSelection`;
2. applies the typed destination's EBTP state;
3. rotates eligible credentials by configured weight before moving to another backend;
4. rebuilds the provider request from immutable canonical bytes;
5. installs provider-owned URI, Host, protocol headers, and credential;
6. uses the shared DNS/SSRF/TLS external-peer path.

OpenAI and OpenAI-compatible services use OpenAI protocol passthrough. Anthropic services translate
OpenAI Chat Completions to the native Messages API and translate the streaming or non-streaming
response back incrementally. The main upstream response always remains on Pingora's streaming path.

Credential RPM/TPM limits use the shared Redis limiter core. A TPM-limited credential requires a
resolvable maximum-output bound. Reservations happen immediately before dispatch; known terminal
usage reconciles the reservation, pre-dispatch cancellation releases it, a gateway-initiated
mid-stream termination also reconciles (the gateway knows it ended the exchange, so the outcome is
not ambiguous — see `skills/04-review/ai-quota/gateway-terminated-stream-settles-its-reservation.md`),
and ambiguous dispatched outcomes conservatively retain it until the fixed window expires.

## Key sources

- `edgion-resources/src/resources/edgion_backend.rs`
- `edgion-resources/src/credentials/resolvers/ai_api_key.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend.rs`
- `edgion-gateway/src/ai/backend_store.rs`
- `edgion-gateway/src/ai/attempt_plan.rs`
- `edgion-gateway/src/ai/anthropic/`
- `edgion-gateway/src/plugins/http/ai_proxy/plugin.rs`
