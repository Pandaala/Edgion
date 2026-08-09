# EdgionAIService

> API: `edgion.io/v1` | Scope: Namespaced | Short name: `eais`

`EdgionAIService` defines **one** provider endpoint, a bounded credential pool, and the provider
model catalog. HTTPRoute references these resources directly from `backendRefs`.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `provider` | Enum | **Required** | `OpenAI`, `OpenAICompatible`, or `Anthropic` |
| `endpoint` | URL | Provider default | Required for `OpenAICompatible`; optional path prefix is supported |
| `allowInsecureHttp` | Boolean | `false` | Allows `http://`; use only in controlled development networks |
| `credentialPool.credentials` | Array | **Required** | One to 16 credentials for this endpoint |
| `credentialPool.redisRef` | String | Conditional | LinkSys Redis reference (`namespace/name`); required when any credential has RPM/TPM limits |
| `credentialPool.onRedisFailure` | Enum | `Deny` | `Deny` fails closed; `Allow` bypasses credential limits during Redis failure |
| `models` | Array | **Required** | One to 256 provider model entries |
| `models[].name` | String | **Required** | Provider model name, at most 512 UTF-8 bytes |
| `models[].aliases` | Array | `[]` | Up to 32 client-facing logical names |
| `models[].pricing` | Object | none | Optional fixed-point USD prices per one million tokens |
| `defaults.streaming` | Boolean | `true` | Rejects client streaming when `false` |
| `defaults.maxOutputTokens` | Integer | none | Fallback output bound for translation, TPM reservation, and estimated cost |

`OpenAI` defaults to `https://api.openai.com`. `Anthropic` defaults to
`https://api.anthropic.com`. There is no timeout field on EdgionAIService — use HTTPRoute
`timeouts` and EBTP `connection.connectTimeout`.

## HTTPRoute backend contract

```yaml
backendRefs:
  - group: edgion.io
    kind: EdgionAIService
    name: primary-provider
    weight: 80
  - group: edgion.io
    kind: EdgionAIService
    name: fallback-provider
    weight: 20
```

Rules:

- `group` must be exactly `edgion.io`, `kind` exactly `EdgionAIService`, and `port` must be absent
- Namespace defaults to the HTTPRoute Namespace; cross-Namespace refs require ReferenceGrant
- `weight: 0` disables a ref
- A rule using AiProxy must have only EdgionAIService forwarding backends
- AI backendRefs reject URL rewrite, redirect, mirror, CORS, external auth, and ExtensionRef filters

## Models and aliases

Clients send a logical model name (for example `chat`). Each backend maps that name independently
to its provider model through `models[].name` or `models[].aliases`. The logical model stays stable
for quota and access logs across failover.

Names and aliases are unique within a service. Optional `pricing` strings are non-negative, support
at most six decimal places, and cannot exceed 1,000,000 USD per million tokens.

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
