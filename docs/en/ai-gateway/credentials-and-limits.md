# Credentials and Limits

Each `EdgionAIService` owns a credential pool for one provider endpoint. Credentials are
Secret-backed; only configured aliases appear in status and access logs.

## Credential parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `credentials[].name` | String | **Required** | Unique non-secret alias, 1–64 ASCII characters |
| `credentials[].weight` | Integer | `1` | Weighted rotation share, 1–1,000,000 |
| `credentials[].secretRef.name` | String | **Required** | Secret containing UTF-8 provider credential data at `api-key` |
| `credentials[].secretRef.namespace` | String | Service Namespace | Cross-Namespace refs require ReferenceGrant |
| `credentials[].limits.requestsPerMinute` | Integer | none | Optional distributed RPM limit |
| `credentials[].limits.tokensPerMinute` | Integer | none | Optional distributed TPM limit |

Never put provider keys in EdgionAIService, EdgionPlugins, HTTPRoute, or application configuration.

## Redis requirements

`credentialPool.redisRef` is required exactly when any credential configures RPM or TPM.
`onRedisFailure` defaults to `Deny` (fail closed). `Allow` explicitly bypasses credential limits
when Redis is unavailable.

## Reservation semantics

- **RPM** reserves one request immediately before dispatch.
- **TPM** reserves estimated input plus the explicit request or service maximum-output bound. A
  TPM-limited credential is invalid when no maximum-output bound can be resolved.
- Known final usage reconciles the reservation.
- A pre-dispatch cancellation releases it.
- A stream the gateway itself ended mid-response — over quota, or quota no longer enforceable —
  also reconciles. The gateway drops that upstream connection itself, so the outcome is known, not
  ambiguous; retaining it would hold the unconsumed remainder against every other tenant sharing
  the credential for the rest of the window.
- A timeout, reset, or other ambiguous result after dispatch retains the full reservation until the
  Redis window expires. That retained reservation protects the rate limit; it is **not** a claim
  that the provider consumed every reserved token. Durable accounting uses the access log.

## Rotation and 429 cooldown

A provider 429 rotates credentials first. A valid Retry-After value creates a bounded cooldown for
that credential; it does not eject sibling credentials or make the current request sleep for the
complete provider interval. A successful request that was already in flight on the same credential
does not shorten the provider cooldown.

Outlier / half-open behavior for AI destinations is credential-isolated — see
[Resilience and Failover](./resilience.md).
