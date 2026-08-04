# Quotas and Cost

## Token quota

Optional AiProxy `tokenQuota` is Redis-backed and keyed by request identity. Omission performs no
quota Redis work.

`tokenQuota` requires `redisRef`, an explicit `key` array, and one to eight limits. Limits use
`scope: identity` or `scope: identityModel`, plus `tokens` and `window`. Defaults are
`onMissingKey: Deny`, `onRedisFailure: Deny`, `flushTokens: 100`, `flushInterval: 1s`, and
`requestTimeout: 500ms`.

Choose one tagged `onExceeded` action:

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

- **Reject** — fail closed when the limit is exceeded.
- **Throttle** — wait at most 1 ms–5 s (bounded by the remaining HTTPRoute request deadline) and
  retry admission once.
- **Downgrade** — requires all limits to use `identityModel`; replaces the logical model and
  repeats backend eligibility and quota admission.

Quota uses fixed windows selected from each gateway instance's wall clock when a request starts.
Keep gateway nodes time-synchronized.

## Estimated cost

Configure optional `models[].pricing` on EdgionAIService:

| Field | Description |
|-------|-------------|
| `inputPerMillionUsd` | Ordinary input-token price |
| `outputPerMillionUsd` | Output-token price |
| `cacheReadInputPerMillionUsd` | Optional; defaults to input price |
| `cacheWriteInputPerMillionUsd` | Optional; defaults to input price |

Estimated cost appears in the access-log `genai` summary and per-attempt ledger when pricing is
configured. `CostWeighted` selection also requires pricing plus an output-token bound for every
otherwise eligible candidate.

Durable billing and usage attribution belong in the access log — see
[Observability](./observability.md).
