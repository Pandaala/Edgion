# Quotas and Cost

Token quota requires `usageObservation: provider`. Observation defaults off; off produces
`notObserved` without token/cost publication. The observation and settlement details below apply
when provider observation is explicitly enabled.

## Token quota

Optional AiProxy `tokenQuota` is Redis-backed and keyed by request identity. Omission performs no
quota Redis work.

`tokenQuota` requires `redisRef`, an explicit `key` array, and one to eight limits. Limits use
`scope: identity` or `scope: identityModel`, plus `tokens` and `window`. Defaults are
`onKeyMiss: Deny`, `onRedisFailure: Deny`, `flushTokens: 100`, `flushInterval: 1s`, and
`requestTimeout: 500ms`.

Business quota checks accumulated consumption without reserving the whole request's input or maximum
output at admission. Active requests flush observed output in batches. Terminal settlement charges
accountable input and output across all dispatched attempts, including retries, rather than only the
last successful attempt. Concurrent requests and delayed flushes permit overshoot; this is not a
strict zero-overshoot budget. Credential TPM is a separate maximum-consumption reservation mechanism;
see [Credentials and Limits](./credentials-and-limits.md#reservation-semantics).

Buckets belong to the AiProxy entry's Namespace, EdgionPlugins name, and entry alias (or entry index
when no alias is supplied). Routes referencing the same entry share buckets; separate entries do
not, even with identical configuration. `key: []` means a global identity bucket within that entry,
not one budget across all gateway entries. There is no separate shared-budget ID field yet.

`identityModel` uses the effective logical model at admission: the client name/alias by default,
after any explicit `modelRoutes` rewrite. Different aliases normally have separate buckets even
when mapped to the same provider model. The 32-alias limit counts patterns; `prefix*` can match
many distinct client names, so it does not imply at most 33 quota buckets. For an aggregate cap
across names, use `identity` within the same entry. `Downgrade` permits only `identityModel`, so
account for alias and wildcard bucket expansion when sizing that policy.

The following action fragments belong inside AiProxy `config.tokenQuota`. See the
[Quick Start](./quick-start.md#complete-example) for resource structure. Before enabling quota,
configure a working LinkSys Redis, `redisRef`, trusted identity `key`, and `limits`. Choose one action:

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

An active request continues charging its request-start window, including after that window
ends. Its Redis lease is refreshed even when no new output units arrive. Shared usage buckets
retain the longest expiry requested by their updates; finishing one request cannot shorten
another active request's retention. Final request markers expire independently.

Allow Redis memory for overlapping retained windows. Each aggregate stores one usage counter;
each charged request has a separate marker. Active updates request retention covering both the
time left until the request-start window ends plus a 60-second grace and the active lease
(maximum HTTP request duration plus `tokenQuota.requestTimeout` plus 60 seconds). Final updates
request the later of that window cleanup deadline and `tokenQuota.requestTimeout` plus 60 seconds
from the update. Each update preserves any later existing bucket expiry, so a bucket may outlive
its last active request. Once updates stop, the remaining TTL expires normally.
There is no fixed fleet-wide memory bound: size for identity/model cardinality, limit count,
window size, request duration, and request rate. Short windows and long request durations can
keep multiple old-window buckets alive. These keys contain accounting state, not response bodies.

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

The decoder degradation and two-malformed-event tolerance below apply to passive OpenAI/OpenAICompatible
usage observation. Anthropic conversion requires translatable events and fails on the first invalid
event under its own rules; it does not acquire that tolerance.

Mid-stream enforcement depends on parsing the upstream's SSE framing: output units are counted
from decoded events. If one **unterminated** event exceeds the decoder's 1 MiB cap, the decoder
gives up parsing for the rest of that stream, and no further event can be counted.

Under `onExceeded: Reject` this **fails closed** — a stream whose framing can no longer be parsed
is a stream whose cost can no longer be bounded, so the gateway ends it rather than let the
upstream keep generating billable tokens unmetered. The injected event reuses the same envelope
the admission path renders when enforcement cannot be performed —
`{"error":{"message":"Token quota enforcement is temporarily unavailable.","type":"api_error","code":"token_quota_unavailable"}}`.

The same gate as above applies: this only happens for a **streaming** request with `tokenQuota`
configured and `onExceeded: Reject`. Passive OpenAI observation without quota does not cut the stream for this framing reason.
Available character estimates may cover only the parsed prefix, reducing precision; another
observation error can still make usage unknown and require unknown settlement. The 1 MiB cap is
a gateway inspection boundary, not a provider protocol guarantee.

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
| `AiSseDecodeDegraded` | SSE decoding gave up on an AI response because one unterminated event exceeded the cap. Available character estimates may cover only the parsed prefix; usage or budget precision is not guaranteed. |
| `AiSseDegradedQuotaTerminated` | A streaming AI response was cut short because quota could no longer be evaluated. Distinct from `AiQuotaOverLimitTerminated`: that one means the limit was reached, this one means the limit could no longer be measured. |
| `AiSseDecodeMalformed` | At least one completed SSE event contained invalid UTF-8 and was excluded from usage parsing. Recorded once per request, whether or not quota enforcement is active. |
| `AiSseMalformedQuotaTerminated` | The second cumulative malformed SSE event caused streaming Reject enforcement to fail closed. This is neither an actual over-limit result nor permanent decoder degradation. |

## JSON observation and unavailable usage

Non-streaming OpenAI Chat JSON usage is extracted incrementally while original
chunks are forwarded. Content strings are validated and discarded by the
observer; the response is not retained for an EOS parse. Selected values become
reported usage only after the complete document is valid. Duplicate selected
keys and replacement of `usage`/details parents use last-wins semantics within
that document, independently of cumulative SSE snapshot merging. A valid complete OpenAI JSON
document without usage can use an input estimate and output estimated from raw JSON characters
rounded up after division by four. JSON wrappers and metadata may contribute; this is not an
exact model-token count.

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

## Responses measurement limits

Responses reuses the existing cumulative quota accumulator and Redis worker. Provider terminal output
totals advance a monotonic watermark; deltas and repeated output do not add separate charges.
Native Responses may report usable totals only at termination. This cannot guarantee per-token
immediate cuts or zero concurrent overshoot. Missing/invalid terminal usage stays unknown, preserves
already charged quota and conservative credential reservation, and is never refunded as zero.
Admission input estimates are provisional capacity estimates, not actual billing: remote history,
hosted resources and hidden reasoning cannot be measured from the submitted input's character count.

Passive observation forwards original malformed or oversized bytes while marking accounting unknown.
With active Reject, broken/oversized SSE framing terminates under the existing failure policy.
A local Responses error uses native type=error, code, message, param and sequence_number, rather
than a Chat error chunk or [DONE]. Its sequence is the checked successor of the highest validated
provider sequence, or zero before one was observed. Overflow suppresses the diagnostic event; this
local marker does not guarantee provider replay/order lineage after a malformed partial event.
For Chat's character-estimation and two-malformed-event tolerance described above, keep the Chat
dialect distinction; those fallback rules do not apply to Responses.

## Native Messages quotas

Messages reuses the same quota worker and terminal owner. Reported input includes cache
creation/read tokens once; cumulative message_delta output advances the existing watermark.
Observation off cannot use tokenQuota, credential TPM or LowestUsage. Required accounting
rejects unsupported versions and recognized composite inference before provider dispatch.
Unknown observations retain existing conservative reservation/settlement behavior. Active
Reject closes with a native error event; Throttle/Downgrade do not acquire that behavior.
Token estimates are provisional admission inputs and configured costs are not a provider invoice.
