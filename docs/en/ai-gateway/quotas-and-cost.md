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

## Mid-stream termination

`tokenQuota` enforcement is not purely an admission-time gate. A **streaming** request already in
flight when its bucket goes over limit is cut short mid-response: the gateway injects one SSE
`error` event using the same envelope the 429 admission rejection uses —
`{"error":{"message":"The token quota has been exceeded.","type":"rate_limit_error","code":"token_quota_exceeded"}}`
— then ends the response. The HTTP status stays `200`: the response was already committed before
the cut, so it cannot change; the client sees the injected event followed by a clean end of
stream, not a dropped connection.

This follows `onExceeded: Reject` only. `Throttle` and `Downgrade` are an operator declaring "do
not hard-fail this user"; that choice is honored for the rest of an admitted stream too, so a
request admitted under either action still runs to completion even if it later goes over limit
mid-stream.

`over_limit()` reflects the identity's **shared** Redis-backed bucket, not the terminated
request's own consumption. Concurrent requests on one identity (a user with several tabs open,
for example) can therefore cut each other's streams: whichever request's next chunk happens to
observe the bucket over limit is the one that gets terminated, essentially arbitrary rather than
proportional to what that particular stream itself consumed — all three tabs can lose their
stream even though only one of them pushed the bucket over.

Alert on:

| EdgionStatus | Code | Meaning |
|--------------|------|---------|
| `AiQuotaOverLimitTerminated` | `999_12` | A streaming AI response was cut short by quota enforcement — a deliberate action, not an upstream failure. |
| `AiQuotaErrorEventDropped` | `999_13` | The injected error event did not fit the per-batch emit budget before the cut; the client saw a bare truncation instead of the diagnostic event. |

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
