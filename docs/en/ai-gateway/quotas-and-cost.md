# Quotas and Cost

## Token quota

Optional AiProxy `tokenQuota` is Redis-backed and keyed by request identity. Omission performs no
quota Redis work.

`tokenQuota` requires `redisRef`, an explicit `key` array, and one to eight limits. Limits use
`scope: identity` or `scope: identityModel`, plus `tokens` and `window`. Defaults are
`onKeyMiss: Deny`, `onRedisFailure: Deny`, `flushTokens: 100`, `flushInterval: 1s`, and
`requestTimeout: 500ms`.

`identityModel` uses the exact client-requested logical model name. The
canonical name and every configured alias therefore have separate buckets,
even when one EdgionBackend maps them to the same provider model. A model can
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

Invalid UTF-8 poisons and drops only the completed SSE event that contains it. Edgion tolerates
the first such event in a response, but the count is cumulative: a valid event does not reset it,
and the second malformed event fails closed under the same streaming + `tokenQuota` +
`onExceeded: Reject` gate. Requests without quota and routes using `Throttle` or `Downgrade`
continue without this termination. The client receives the generic `token_quota_unavailable`
envelope above; malformed provider bytes and other internal details are never exposed in the
error description.

Alert on:

| Error diagnostic | Meaning |
|------------------|---------|
| `AiQuotaOverLimitTerminated` | A streaming AI response was cut short by quota enforcement — a deliberate action, not an upstream failure. |
| `AiQuotaErrorEventDropped` | The injected error event did not fit the per-batch emit budget before the cut; the client saw a bare truncation instead of the diagnostic event. |
| `AiSseDecodeDegraded` | SSE decoding gave up on an AI response because one unterminated event exceeded the cap. On its own this costs accounting precision, not correctness: reported usage falls back to the character estimate. |
| `AiSseDegradedQuotaTerminated` | A streaming AI response was cut short because quota could no longer be evaluated. Distinct from `AiQuotaOverLimitTerminated`: that one means the limit was reached, this one means the limit could no longer be measured. |
| `AiSseDecodeMalformed` | At least one completed SSE event contained invalid UTF-8 and was excluded from usage parsing. Recorded once per request, whether or not quota enforcement is active. |
| `AiSseMalformedQuotaTerminated` | The second cumulative malformed SSE event caused streaming Reject enforcement to fail closed. This is neither an actual over-limit result nor permanent decoder degradation. |

## JSON observation and unavailable usage

Non-streaming OpenAI Chat JSON usage is extracted incrementally while original
chunks are forwarded. Content strings are validated and discarded by the
observer; the response is not retained for an EOS parse. Selected values become
reported usage only after the complete document is valid. Duplicate selected
keys and replacement of `usage`/details parents use last-wins semantics within
that document, independently of cumulative SSE snapshot merging.

Inspection has fixed bounds: 64 MiB source and decoded bytes, nesting depth 64,
256-byte decoded keys, 1,538-byte raw keys, 64-byte number spellings, 131,072
lexical events, 256 Mi work units, and 8 KiB retained heap capacity. These are
observation limits, not response/download size limits. Malformed or incomplete
JSON, exhausted inspection budgets, unsupported content encoding, or token
arithmetic overflow produce unknown usage. Passive observation still forwards
original response bytes; it never invents actual zero usage or final cost.

The gateway requests upstream `Accept-Encoding: identity` and checks all raw
response encoding fields before conversion or response-header edits. An encoded
response on an admitted streaming `tokenQuota` + `Reject` request fails before
header commitment with HTTP 503 and the existing `token_quota_unavailable` JSON
envelope. It does not append cleartext SSE into compressed bytes. `Throttle`
and `Downgrade` do not acquire this hard rejection. The existing Anthropic
converter retains its own error handling when no Reject precommit applies.

Unknown usage retains conservative credential reservations even at EOS or a
gateway cut. Business quota finalizes as `unresolved` after a successful
conservative flush; Redis failure or deadline expiry can still yield `failed`
or `timed_out`. It flushes the highest
observed, acknowledged or sent watermark without refunding a possibly committed
Redis apply. The worker still terminates; a zero observed watermark is not a
claim of zero actual consumption. Valid estimated client-disconnect and gateway-cut
accounting remain supported; transport ambiguity and usage quality are separate.
Inspect `AiUsageObservationUnavailable`; with business quota configured,
arithmetic overflow additionally uses `AiQuotaReconciliationFailed`.
Known attempt rows remain in the access log.

## Estimated cost

Configure optional `models[].pricing` on EdgionBackend:

| Field | Description |
|-------|-------------|
| `inputPerMillionUsd` | Ordinary input-token price |
| `outputPerMillionUsd` | Output-token price |
| `cacheReadInputPerMillionUsd` | Optional; defaults to input price |
| `cacheWriteInputPerMillionUsd` | Optional; defaults to input price |

Estimated cost appears in the access-log `genai` summary and per-attempt ledger when pricing is
configured and usage is reported or validly estimated. Unknown/unobserved usage omits cost. `CostWeighted` selection also requires pricing plus an output-token bound for every
otherwise eligible candidate.

Durable billing and usage attribution belong in the access log — see
[Observability](./observability.md).
