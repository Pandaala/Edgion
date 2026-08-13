---
name: edgion-backend-features
description: Canonical EdgionBackend credential-pool schema, AI HTTPRoute backend contract, AiProxy selection, defaults, status, and complete examples.
---

# EdgionBackend resource

> API: `edgion.io/v1` | Scope: Namespaced | Short name: `eb`
> Internal processing: [../../01-architecture/05-resources/22-edgion-backend.md](../../01-architecture/05-resources/22-edgion-backend.md)

`EdgionBackend` defines one provider endpoint, a bounded credential pool, and the provider model
catalog. An HTTPRoute references these resources directly from `backendRefs`. The route is the only
source of backend candidates, declaration order, and weights; rule-level `AiProxy` stores only the
selection algorithm and AI request behavior.

## Full schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackend
metadata:
  name: chat-provider
  namespace: production
spec:
  ai:
    provider: OpenAICompatible
    endpoint: https://llm.example.com/openai
    allowInsecureHttp: false
    credentialPool:
      redisRef: production/ai-quota-redis
      onRedisFailure: Deny
      credentials:
        - name: account-primary
          weight: 100
          secretRef:
            name: chat-provider-primary
          limits:
            requestsPerMinute: 500
            tokensPerMinute: 200000
        - name: account-secondary
          weight: 50
          secretRef:
            name: chat-provider-secondary
    models:
      - name: provider-chat-v2
        aliases:
          - chat
          - chat-v2-*
        public: true
        pricing:
          inputPerMillionUsd: "1"
          outputPerMillionUsd: "4"
    defaults:
      streaming: true
      maxOutputTokens: 4096
status:
  conditions: []
```

Input handling differs by config-center mode. On Kubernetes, requests using
`fieldValidation=Strict` reject undeclared fields throughout `spec.ai`; Warn/Ignore clients may
allow the API server to prune a misspelled optional field into a silent no-op. Filesystem and etcd
input is deserialized directly and always rejects unknown fields in the strict `spec` tree.
Resolved Secret objects are Controller-filled transport fields; they are excluded from the CRD
schema and redacted from Admin configuration output.

## `spec` fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `spec.ai.provider` | Enum | Yes | — | `OpenAI`, `OpenAICompatible`, or `Anthropic` |
| `spec.ai.endpoint` | URL string | Provider-dependent | Provider default | Absolute HTTP(S) URL with optional path prefix. Required for `OpenAICompatible`. User information, query, and fragment are rejected |
| `spec.ai.allowInsecureHttp` | Boolean | No | `false` | Allows an explicit `http://` endpoint; use only in a controlled development network |
| `spec.ai.credentialPool` | Object | Yes | — | One to 16 credentials for the same endpoint and provider protocol |
| `spec.ai.models` | Array | Yes | — | One to 256 canonical model entries |
| `spec.ai.defaults` | Object | No | `{streaming: true}` | Provider behavior defaults. `maxOutputTokens` has no implicit value |

`OpenAI` defaults to `https://api.openai.com`; `Anthropic` defaults to
`https://api.anthropic.com`. `OpenAICompatible` requires `spec.ai.endpoint`.

### `spec.ai.credentialPool`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `redisRef` | String | Conditional | — | LinkSys Redis reference in `namespace/name` form. Required exactly when any credential has an RPM or TPM limit |
| `onRedisFailure` | Enum | No | `Deny` | `Deny` fails closed; `Allow` explicitly bypasses credential limits when Redis is unavailable |
| `credentials` | Array | Yes | — | One to 16 credential entries |
| `credentials[].name` | String | Yes | — | Unique non-secret alias matching `[A-Za-z0-9][A-Za-z0-9._-]{0,63}` |
| `credentials[].weight` | Integer | No | `1` | Weighted rotation share, from 1 through 1,000,000 |
| `credentials[].secretRef` | Object | Yes | — | Kubernetes Secret reference containing UTF-8 `api-key` data |
| `credentials[].limits` | Object | No | — | Optional per-credential distributed limits |
| `credentials[].limits.requestsPerMinute` | Integer | No | — | RPM limit, from 1 through `2^53-1` |
| `credentials[].limits.tokensPerMinute` | Integer | No | — | TPM limit, from 1 through `2^53-1` |

A `secretRef` defaults to the EdgionBackend Namespace. A cross-Namespace reference requires a
matching ReferenceGrant. The Secret type may be `Opaque` or omitted. Never put provider keys in
the EdgionBackend, EdgionPlugins, HTTPRoute, or application configuration.

Secret resolution is partial-good. At least one usable credential activates the backend and omits
invalid entries. No usable credential deactivates the backend. Credential aliases may appear in
bounded status and access-log records; Secret values never do.

Credential selection is hierarchical: AiProxy first chooses an HTTPRoute backend, then rotates
eligible credentials within that backend by weight before moving to another backend. A 429 may
cool down that credential without ejecting its siblings.

RPM reserves one request immediately before dispatch. TPM reserves estimated input plus the
explicit request or backend maximum-output bound. Therefore a TPM-limited credential is invalid
when no maximum-output bound can be resolved. Known terminal usage reconciles the reservation, and
so does a stream the gateway itself terminated mid-response (its outcome is known, not ambiguous);
only genuinely ambiguous dispatched outcomes conservatively retain the full reservation until the
window expires.

### `spec.ai.models`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | String | Yes | — | Provider model name, at most 512 UTF-8 bytes |
| `aliases` | String array | No | `[]` | Up to 32 exact names or suffix wildcards (`prefix*`), each at most 512 UTF-8 bytes |
| `public` | Boolean | No | `true` | Expose the canonical name and direct aliases through route-scoped `/v1/models` |
| `pricing` | Object | No | — | Estimated USD prices per one million tokens |
| `pricing.inputPerMillionUsd` | Decimal string | Yes with `pricing` | — | Ordinary input-token price |
| `pricing.outputPerMillionUsd` | Decimal string | Yes with `pricing` | — | Output-token price |
| `pricing.cacheReadInputPerMillionUsd` | Decimal string | No | Input price | Cache-read input-token price |
| `pricing.cacheWriteInputPerMillionUsd` | Decimal string | No | Input price | Cache-creation/write input-token price |

Names and aliases are unique across the complete backend. Price strings are non-negative, support
at most six decimal places, and cannot exceed 1,000,000 USD per million tokens.
Wildcard aliases contain exactly one trailing `*` after a non-empty prefix.
Exact lookup wins; overlapping wildcards use the longest literal prefix.

### `spec.ai.defaults`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `streaming` | Boolean | No | `true` | When `false`, a request containing `stream: true` is rejected |
| `maxOutputTokens` | Positive integer | No | — | Backend fallback when the request does not provide an output-token bound |

There is no `spec.ai.defaults.timeout`. Configure the total and per-attempt deadlines with
`HTTPRoute.timeouts`; configure connection establishment with
EdgionBackendTrafficPolicy `connection.connectTimeout`.

## HTTPRoute AI backend contract

An AI rule references EdgionBackend directly:

```yaml
backendRefs:
  - group: edgion.io
    kind: EdgionBackend
    name: primary-provider
    weight: 80
  - group: edgion.io
    kind: EdgionBackend
    name: fallback-provider
    weight: 20
```

Rules:

- `group` is exactly `edgion.io`, `kind` is exactly `EdgionBackend`, and `port` is absent
- Namespace defaults to the HTTPRoute Namespace; cross-Namespace refs require ReferenceGrant
- `weight: 0` disables a ref
- A rule using AiProxy contains only EdgionBackend forwarding backends
- Exactly one enabled AiProxy is attached at rule level through an EdgionPlugins ExtensionRef
- BackendRef filters allow only `RequestHeaderModifier` and `ResponseHeaderModifier` in v1
- `RequestHeaderModifier` is rebuilt per attempt; `ResponseHeaderModifier` applies only to the
  final non-retried response
- AI backendRefs reject URL rewrite, redirect, mirror, CORS, external auth, and ExtensionRef filters

Missing, denied, or unsupported refs keep their Gateway API weighted invalid-traffic share. If an
invalid slot is selected, forwarding returns 500 without AI failover.

## `AiProxy` configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ai-proxy
  namespace: production
spec:
  requestPlugins:
    - type: AiProxy
      config:
        backendSelection:
          type: Weighted
        injectUsageTracking: true
        onUnknownModel: reject
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `backendSelection.type` | Enum | No | `Weighted` | `Weighted`, `Priority`, `LowestLatency`, `LowestUsage`, or `CostWeighted` |
| `injectUsageTracking` | Boolean | No | `true` | Requests provider usage for streaming passthrough while retaining independent access-log accounting |
| `onUnknownModel` | Enum | No | `reject` | `reject` fails unknown models; `passthrough` is limited to OpenAI-compatible candidates with `Weighted` or `Priority` |
| `modelRoutes` | Array | No | `[]` | Up to 64 bounded model/header matches with canonical rewrite and typed candidate overrides |
| `tokenQuota` | Object | No | — | Optional Redis-backed logical-model token quota; omission has no Redis/task overhead |

Selection always scans only valid positive-weight route candidates that support the request's
logical model:

| Type | Behavior |
|------|----------|
| `Weighted` | Uses HTTPRoute weights directly |
| `Priority` | Uses HTTPRoute backend declaration order as strict primary-to-fallback order |
| `LowestLatency` | Chooses the lowest rolling p50 time to first token; route weight breaks exact ties |
| `LowestUsage` | Chooses the lowest recent charged-token usage divided by route weight |
| `CostWeighted` | Uses route weight and inverse-square estimated request cost |

`CostWeighted` requires pricing and an output bound for every otherwise eligible candidate. The base
AiProxy selection never embeds a provider list or retry attempts. A `modelRoutes` action may
explicitly identify existing typed route slots and override their effective weights; the HTTPRoute
declarations remain authoritative and invalid shares are preserved.

### Token quota

`tokenQuota` requires `redisRef`, an explicit `key` array, and one to eight `limits`. Each limit has
`scope: identity` or `scope: identityModel`, `tokens`, and `window`. Defaults are
`onMissingKey: Deny`, `onRedisFailure: Deny`, `flushTokens: 100`, `flushInterval: 1s`, and
`requestTimeout: 500ms`.

`identityModel` partitions by the exact requested logical name. A canonical
name and each configured alias use different buckets even when they resolve to
the same provider model (up to 33 declared names per model). Use `identity` for
a cap shared across names, or avoid aliases on quota-controlled models.

`onExceeded` is one tagged action:

```yaml
onExceeded:
  type: Reject
```

```yaml
onExceeded:
  type: Throttle
  maxDelay: 250ms
```

```yaml
onExceeded:
  type: Downgrade
  model: chat-economy
```

Throttle permits `maxDelay` from 1 ms through 5 s and retries admission once within the remaining
HTTPRoute request deadline. Downgrade requires every limit to use `identityModel`, replaces the
logical model, and repeats eligibility and quota admission.

## Status

| Condition | Meaning |
|-----------|---------|
| `Accepted` | Static provider, endpoint, pool, model, pricing, and default validation |
| `ResolvedRefs` | Every credential Secret reference resolved; any missing, denied, or invalid Secret makes this condition false |
| `PartiallyInvalid` | Some credentials were omitted while a usable subset remains active; reason `InvalidCredentials` contains bounded aliases and fixed reason enums |

All conditions carry `observedGeneration`. Secret rotation and corrected refs affect fresh requests
after normal Controller-to-Gateway synchronization without restarting either binary.

## Complete configuration example

Replace the Secret values and ensure `main-gateway` and `production/ai-redis` exist.

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: primary-key
  namespace: production
type: Opaque
stringData:
  api-key: replace-with-primary-provider-key
---
apiVersion: v1
kind: Secret
metadata:
  name: fallback-key
  namespace: production
type: Opaque
stringData:
  api-key: replace-with-fallback-provider-key
---
apiVersion: edgion.io/v1
kind: EdgionBackend
metadata:
  name: primary-provider
  namespace: production
spec:
  ai:
    provider: OpenAI
    credentialPool:
      credentials:
        - name: primary-account
          secretRef:
            name: primary-key
    models:
      - name: provider-chat-v2
        aliases: [chat]
    defaults:
      streaming: true
      maxOutputTokens: 4096
---
apiVersion: edgion.io/v1
kind: EdgionBackend
metadata:
  name: fallback-provider
  namespace: production
spec:
  ai:
    provider: Anthropic
    credentialPool:
      credentials:
        - name: fallback-account
          secretRef:
            name: fallback-key
    models:
      - name: claude-example
        aliases: [chat]
    defaults:
      streaming: true
      maxOutputTokens: 4096
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ai-proxy
  namespace: production
spec:
  requestPlugins:
    - type: AiProxy
      config:
        backendSelection:
          type: Priority
        injectUsageTracking: true
        onUnknownModel: reject
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: ai-chat-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - ai.example.com
  rules:
    - name: chat
      matches:
        - path:
            type: PathPrefix
            value: /v1/chat/completions
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: ai-proxy
      backendRefs:
        - group: edgion.io
          kind: EdgionBackend
          name: primary-provider
          weight: 80
        - group: edgion.io
          kind: EdgionBackend
          name: fallback-provider
          weight: 20
      timeouts:
        backendRequest: 30s
        request: 90s
      retry:
        attempts: 2
        backoff: 10ms
        codes: [429, 502, 503, 504]
```

## Related documentation

- [HTTPRoute](04-httproute.md)
- [EdgionBackendTrafficPolicy](20-edgion-backend-traffic-policy.md)
- [EdgionBackend internals](../../01-architecture/05-resources/22-edgion-backend.md)
