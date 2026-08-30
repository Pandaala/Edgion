# EdgionBackend

> API: `edgion.io/v1` | Scope: Namespaced | Short name: `eb`

`EdgionBackend` defines **one** provider endpoint, a bounded credential pool, and the provider
model catalog. HTTPRoute references these resources directly from `backendRefs`.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `spec.ai.provider` | Enum | **Required** | `OpenAI`, `OpenAICompatible`, or `Anthropic` |
| `spec.ai.endpoint` | URL | Provider default | Required for `OpenAICompatible`; optional path prefix is supported |
| `spec.ai.allowInsecureHttp` | Boolean | `false` | Allows `http://`; use only in controlled development networks |
| `spec.ai.credentialPool.credentials` | Array | **Required** | One to 16 credentials for this endpoint |
| `spec.ai.credentialPool.redisRef` | String | Conditional | LinkSys Redis reference (`namespace/name`); required when any credential has RPM/TPM limits |
| `spec.ai.credentialPool.onRedisFailure` | Enum | `Deny` | `Deny` fails closed; `Allow` bypasses credential limits during Redis failure |
| `spec.ai.models` | Array | **Required** | One to 256 provider model entries |
| `spec.ai.models[].name` | String | **Required** | Provider model name, at most 512 UTF-8 bytes |
| `spec.ai.models[].aliases` | Array | `[]` | Up to 32 exact names or suffix wildcards (`prefix*`) |
| `spec.ai.models[].public` | Boolean | `true` | Include the canonical name and aliases in route-scoped `/v1/models` |
| `spec.ai.models[].pricing` | Object | none | Optional fixed-point USD prices per one million tokens |
| `spec.ai.defaults.streaming` | Boolean | `true` | Rejects client streaming when `false` |
| `spec.ai.defaults.maxOutputTokens` | Integer | none | Fallback output bound for translation, TPM reservation, and estimated cost |

`OpenAI` defaults to `https://api.openai.com`. `Anthropic` defaults to
`https://api.anthropic.com`. There is no timeout field on EdgionBackend — use HTTPRoute
`timeouts` and EBTP `connection.connectTimeout`.

Kubernetes clients using strict field validation reject unknown fields throughout `spec.ai`,
including misspellings. Clients using warn/ignore validation may let the API server prune an
undeclared field. Filesystem and etcd configuration always apply strict deserialization.

## HTTPRoute backend contract

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

- `group` must be exactly `edgion.io`, `kind` exactly `EdgionBackend`, and `port` must be absent
- Namespace defaults to the HTTPRoute Namespace; cross-Namespace refs require ReferenceGrant
- `weight: 0` disables a ref
- A rule using AiProxy must have only EdgionBackend forwarding backends
- AI backendRefs reject URL rewrite, redirect, mirror, CORS, external auth, and ExtensionRef filters

## Models and aliases

Clients send a logical model name (for example `chat`). Each backend maps that name independently
to its provider model through `spec.ai.models[].name` or `spec.ai.models[].aliases`. The logical model stays stable
for quota and access logs across failover.

Names and aliases are unique within a backend. Optional `pricing` strings are non-negative, support
at most six decimal places, and cannot exceed 1,000,000 USD per million tokens.
Exact aliases take precedence over suffix wildcards, and the longest matching
wildcard prefix wins. `*` alone and embedded wildcards are invalid.

## Status

| Condition | Meaning |
|-----------|---------|
| `Accepted=False/Invalid` | Invalid endpoint, pool, model, pricing, or default |
| `ResolvedRefs=False` | At least one credential Secret is missing, denied, or invalid |
| `PartiallyInvalid=True/InvalidCredentials` | Usable subset is active; omitted aliases appear in a bounded reason |

Secret resolution is partial-good: if some credentials resolve, Edgion serves with those entries.
`ResolvedRefs` remains false until every reference resolves. If none resolve, the service is
inactive and requests fail closed with 503.

## Related

- [Credentials and Limits](./credentials-and-limits.md)
- [Providers](./providers.md)
- [Quick Start complete example](./quick-start.md#complete-example)
