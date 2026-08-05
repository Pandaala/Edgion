---
name: resource-edgion-ai-service
description: EdgionAIService credential-pool resolution, route dependencies, conf_sync transport, Gateway snapshots, and AiProxy attempt consumption.
---

# EdgionAIService resource

> Common flow: [00-resource-flow.md](00-resource-flow.md)
> External schema and complete examples: [../../02-features/03-resources/22-edgion-ai-service.md](../../02-features/03-resources/22-edgion-ai-service.md)

`EdgionAIService` is one namespaced provider endpoint, model catalog, pricing table, and bounded
credential pool. It is a typed `HTTPRoute.backendRef`; it is not selected by a name stored in
`AiProxy`. The route is the only source of candidate backends, declaration order, and weights.
`AiProxy` contributes only AI request semantics and `backendSelection`.

## Controller processing

`EdgionAIServiceHandler` follows the normal serialized workqueue flow:

1. `preparse` runs `EdgionAIServiceSpec::validate`. Structural errors become
   `Accepted=False/Invalid`.
2. `parse` clears all previous Secret dependency edges and resolved credential material before
   registering every credential's `secretRef`. This full replacement is required for
   delete-create coalescing.
3. Each credential is independently authorized through `ReferenceGrant` and resolved through
   `AiApiKeyResolver`. Dependencies remain registered when resolution fails so later Secret or
   ReferenceGrant changes self-heal.
4. Secret resolution is partial-good. At least one usable credential activates the service;
   unusable entries are omitted from conf_sync runtime material. Zero usable credentials
   deactivates the service.
5. `on_delete` clears Secret edges and requeues typed dependents through their own workqueues.

An EdgionAIService apply or delete requeues referencing HTTPRoutes and
EdgionBackendTrafficPolicies through `ai_service_ref_mgr`. HTTPRoute reconcile registers terminal
AI backends after delegation flattening. EBTP reconcile registers typed policy targets. No handler
calls another handler directly.

## Secret and transport boundaries

Each `credentialPool.credentials[]` entry flattens the shared `SecretSlot`:

- `secretRef` is operator-facing and structural in the CRD;
- resolved `secret` is `#[schemars(skip)]`, remains serde-visible for conf_sync, and uses the
  shared Admin redaction adapter;
- `name` is a stable, non-secret alias used for status and bounded access-log attribution;
- Controller and Gateway logs never contain resource bodies, credential values, or provider URLs.

The Gateway generic config-client endpoint is `/configclient/edgionaiservice`. Under the Admin
serialization guard every resolved Secret is rendered as `[redacted]`; conf_sync still carries the
real value over its trusted channel.

## Status

- `Accepted=False/Invalid` reports structural validation errors.
- `ResolvedRefs=True` means at least one credential is usable.
- `PartiallyInvalid=True/InvalidCredentials` reports a bounded count plus credential aliases and
  fixed reason enums for omitted credentials.
- `ResolvedRefs=False` means no credential is usable, so the Gateway deactivates the service.

Route `ResolvedRefs` is intentionally separate: an existing and authorized EdgionAIService is a
resolved backendRef even when its own credentials are unresolved. Requests then fail closed with a
generic 503 until the service becomes active.

## Gateway runtime

`AiServiceStore` is the sole writer and publishes one immutable ArcSwap snapshot. Cold-path
compilation:

- revalidates the spec;
- decomposes the endpoint into dial domain, port, TLS/SNI, Host authority, and path prefix;
- retains only usable credentials, their weights, optional RPM/TPM limits, and stable aliases;
- builds a collision-free canonical-model and alias table (at most 256 models and 32 aliases per
  model);
- parses optional prices into fixed-point integers;
- retains `defaults.streaming` and optional `defaults.maxOutputTokens`.

Equivalent updates reuse stable runtime and backend-policy handles. Invalid or zero-credential
updates remove the previous activatable generation rather than serving stale credentials. Each
request captures one service-store snapshot, so hot reload cannot mix service generations across
attempts.

`defaults.timeout` does not exist. `HTTPRoute.timeouts` owns request and per-attempt deadlines;
EdgionBackendTrafficPolicy `connection.connectTimeout` owns connection establishment.

## Route and attempt consumption

The compiled HTTPRoute rule supplies every candidate as a typed EdgionAIService backendRef.
AiProxy reads the canonical OpenAI request once, resolves a request-stable logical model, applies
quota, and stores a bounded attempt plan. It does not store backend names or mutate an upstream
jump slot.

For each upstream attempt the Gateway:

1. selects an eligible route backend with `backendSelection`;
2. applies the typed destination's EBTP state;
3. rotates eligible credentials by configured weight before moving to another service;
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

- `edgion-resources/src/resources/edgion_ai_service.rs`
- `edgion-resources/src/credentials/resolvers/ai_api_key.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_ai_service.rs`
- `edgion-gateway/src/ai/service_store.rs`
- `edgion-gateway/src/ai/attempt_plan.rs`
- `edgion-gateway/src/ai/anthropic/`
- `edgion-gateway/src/plugins/http/ai_proxy/plugin.rs`
