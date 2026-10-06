# Credentials and Limits

Each `EdgionBackend` owns a credential pool for one provider endpoint. Credentials are
Secret-backed; only configured aliases appear in status and access logs.

## Credential parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `credentials[].name` | String | **Required** | Unique non-secret alias, 1–64 ASCII characters |
| `credentials[].weight` | Integer | `1` | Weighted rotation share, 1–1,000,000 |
| `credentials[].secretRef.name` | String | **Required** | Secret containing UTF-8 provider credential data at `api-key` |
| `credentials[].secretRef.namespace` | String | EdgionBackend Namespace | Cross-Namespace refs require ReferenceGrant |
| `credentials[].limits.requestsPerMinute` | Integer | none | Optional distributed RPM limit |
| `credentials[].limits.tokensPerMinute` | Integer | none | Optional distributed TPM limit |

Never put provider keys in EdgionBackend, EdgionPlugins, HTTPRoute, or application configuration.

## Redis requirements

`credentialPool.redisRef` is required exactly when any credential configures RPM or TPM.
`onRedisFailure` defaults to `Deny` (fail closed). `Allow` explicitly bypasses credential limits
when Redis is unavailable.

## Reservation semantics

Any credential with TPM configured requires EdgionBackend `defaults.maxOutputTokens`, even if
clients always supply an output bound. Configuration validation requires this fallback; runtime
resolution still prefers the request value.

- **RPM** reserves one request immediately before dispatch.
- **TPM** reserves estimated input plus the request-declared output value, falling back to
  `defaults.maxOutputTokens` only when absent. A
  TPM-limited credential is invalid when no maximum-output bound can be resolved.
- Known final usage reconciles the reservation.
- A pre-dispatch cancellation releases it.
- A stream ended by the gateway — over quota, or quota no longer enforceable — reconciles only
  with reported or valid estimated usage. Closing the upstream removes transport ambiguity, not
  usage uncertainty; unknown usage retains the reservation even at EOS.
- A timeout, reset, or other ambiguous result after dispatch retains the full reservation until the
  Redis window expires. That retained reservation protects the rate limit; it is **not** a claim
  that the provider consumed every reserved token. Durable accounting uses the access log.

`defaults.maxOutputTokens` is not an enforced output ceiling; an explicit request value overrides it.
Credential RPM/TPM protects provider credentials. Business `tokenQuota` uses separate cumulative
buckets; see [Quotas and Cost](./quotas-and-cost.md).

## Rotation and 429 cooldown

A provider 429 rotates credentials first. A valid Retry-After value creates a bounded cooldown for
that credential; it does not eject sibling credentials or make the current request sleep for the
complete provider interval. A successful request that was already in flight on the same credential
does not shorten the provider cooldown.

Outlier / half-open behavior for AI destinations is credential-isolated — see
[Resilience and Failover](./resilience.md).

## Unary conversion resource limits

Selected non-streaming Chat-to-Anthropic conversion validates and serializes one
bounded provider document before committing the final response. Configure its
process-wide limits in the Gateway YAML, not in an EdgionBackend or AiProxy entry.
The following is a process configuration fragment:

```yaml
ai:
  conversion:
    chat_to_anthropic:
      unary:
        max_input_size: 1MiB
        max_output_size: 1MiB
        hold_timeout: 120s
        memory_budget: 256MiB
        max_json_depth: 64
        max_json_nodes: 16384
        max_key_size: 256B
        max_source_events: 4096
        max_metadata_size: 64KiB
```

| Field | Default | Valid domain / meaning |
|-------|---------|------------------------|
| `max_input_size` | `1MiB` | 1 byte through `16MiB`; provider JSON bytes |
| `max_output_size` | `1MiB` | 1 byte through `16MiB`; serialized Chat completion bytes |
| `hold_timeout` | `120s` | `1ms` through `1h`; absolute ceiling from Hold activation, tightened by existing absolute deadlines |
| `memory_budget` | `256MiB` | Derived reservation `R` through `16GiB`; one process-wide admission pool |
| `max_json_depth` | `64` | Integer `1..=64` |
| `max_json_nodes` | `16384` | Integer `1..=16384`; includes unknown fields and overwritten duplicates |
| `max_key_size` | `256B` | 1 byte through `256B`; decoded JSON key bytes |
| `max_source_events` | `4096` | Integer `4..=65536`; source tasks, including empty body events |
| `max_metadata_size` | `64KiB` | `8KiB` through `64KiB`; cumulative retained head/trailer metadata, also at most 64 fields per projection |

All nine fields require a Gateway restart. Omitted fields use defaults; unknown
keys in any AI conversion subtree, zero, out-of-range values and arithmetic
overflow fail startup. The pool must fund at least one computed reservation;
startup reports the required byte count when it cannot. No dedicated CLI flags or
environment overrides exist. Routes and overlapping plugin generations share one
pool. These fields neither enable conversion nor alter ordinary HTTP, native AI
or SSE streaming.

Defaults derive a **25MiB reservation and ten concurrent slots**. The reservation
changes with the configured safety bounds and covers conversion-owned buffers,
parser/DOM/temporary/output state and bounded retained tasks/compact metadata.
It is **not a process RSS ceiling**: original request/header storage, normal
transport/socket/TLS buffers, unrelated plugins, allocator overhead and
fragmentation are excluded. It does not guarantee physical allocation success.
Capacity returns after both exchange cleanup and the last output clone drop.

Exhausted admission or an active incompatible Hold claim returns fixed **503**
before any provider dispatch. Invalid/truncated provider JSON, unsupported success
media/encoding or conversion/resource overflow returns fixed **502** before final
headers or body; expiry of the configured absolute Hold ceiling returns fixed
**504**. Existing shorter absolute deadlines still apply. For selected unary conversion, upstream failure or timeout before provider headers also uses the
fixed Chat 502/504 envelope. True provider error statuses and explicit plugin denials retain
their existing handling.

Fully validated checked provider usage remains known if later serialization or
delivery fails. Invalid or incomplete JSON has unknown usage; fixed errors never
produce output-character estimates. Accounting reuses the translator's usage and
keeps provider status separate from the final client outcome.
