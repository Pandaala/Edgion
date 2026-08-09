# Quotas and Cost

## Token quota

Optional AiProxy `tokenQuota` is Redis-backed and keyed by request identity. Omission performs no
quota Redis work.

`tokenQuota` requires `redisRef`, an explicit `key` array, and one to eight limits. Limits use
`scope: identity` or `scope: identityModel`, plus `tokens` and `window`. Defaults are
`onMissingKey: Deny`, `onRedisFailure: Deny`, `flushTokens: 100`, `flushInterval: 1s`, and
`requestTimeout: 500ms`.

`identityModel` uses the exact client-requested logical model name. The
canonical name and every configured alias therefore have separate buckets,
even when one EdgionAIService maps them to the same provider model. A model can
have up to 32 aliases, so one identity can consume the configured limit once
for each of up to 33 declared names. Use `identity` for one cap shared across
all names, or avoid aliases on quota-controlled models. `Downgrade` requires
`identityModel`, so alias sets must be included when sizing its limits.

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

Streaming provider usage objects are treated as cumulative snapshots. When an OpenAI-compatible
provider emits usage on multiple chunks, Edgion keeps the highest observed value for each token
component; a later lower or out-of-order snapshot cannot reduce quota reconciliation, access-log
usage, or estimated cost. OpenAI's usual final-only usage frame is unchanged.

## Mid-stream termination

`tokenQuota` enforcement is not purely an admission-time gate. A **streaming** request already in
flight when its bucket goes over limit is cut short mid-response: the gateway injects one SSE
`error` event using the same envelope the 429 admission rejection uses —
`{"error":{"message":"The token quota has been exceeded.","type":"rate_limit_error","code":"token_quota_exceeded"}}`
— then ends the response. The HTTP status stays `200`: the response was already committed before
the cut, so it cannot change; the client sees the injected event followed by a clean end of
stream, not a dropped connection.

The injected event is preceded by a blank line. Where an upstream ends a chunk is not something
the gateway controls, so the injection can land in the middle of a half-delivered event; the
blank line closes that event first, which is what guarantees the error is dispatched as its own
event rather than being concatenated onto truncated JSON and dropped by the parser. A conforming
SSE client never observes the extra blank line — with nothing buffered it dispatches nothing.

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

### When the stream can no longer be measured

Mid-stream enforcement depends on parsing the upstream's SSE framing: output units are counted
from decoded events. If one **unterminated** event exceeds the decoder's 1 MiB cap, the decoder
gives up parsing for the rest of that stream, and no further event can be counted.

Under `onExceeded: Reject` this **fails closed** — a stream whose framing can no longer be parsed
is a stream whose cost can no longer be bounded, so the gateway ends it rather than let the
upstream keep generating billable tokens unmetered. The injected event reuses the same envelope
the admission path renders when enforcement cannot be performed —
`{"error":{"message":"Token quota enforcement is temporarily unavailable.","type":"api_error","code":"token_quota_unavailable"}}`.

The same gate as above applies: this only happens for a **streaming** request with `tokenQuota`
configured and `onExceeded: Reject`. A request without a quota is never cut for a framing reason —
there, degradation only downgrades reported usage to the character estimate, as it always has.

The 1 MiB cap is sized so a legitimate provider does not reach it. A normal streaming event is a
few hundred bytes; 1 MiB is reached only by an upstream that emits an entire completion as one
event, or one that never emits an event boundary at all.

Alert on:

| EdgionStatus | Code | Meaning |
|--------------|------|---------|
| `AiQuotaOverLimitTerminated` | `999_12` | A streaming AI response was cut short by quota enforcement — a deliberate action, not an upstream failure. |
| `AiQuotaErrorEventDropped` | `999_13` | The injected error event did not fit the per-batch emit budget before the cut; the client saw a bare truncation instead of the diagnostic event. |
| `AiSseDecodeDegraded` | `999_14` | SSE decoding gave up on an AI response because one unterminated event exceeded the cap. On its own this costs accounting precision, not correctness: reported usage falls back to the character estimate. |
| `AiSseDegradedQuotaTerminated` | `999_15` | A streaming AI response was cut short because quota could no longer be evaluated. Distinct from `AiQuotaOverLimitTerminated`: that one means the limit was reached, this one means the limit could no longer be measured. |

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
