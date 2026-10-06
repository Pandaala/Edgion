---
name: sse-malformed-events-must-not-disable-enforcement
description: Use when reviewing malformed SSE handling on the OpenAI quota path; preserves the cumulative two-event fail-close threshold, enforcement gates, precedence, observability, and WS-06 migration contract.
status: fixed
finding_id: D1b
closed: 2026-08-11
knowledge_type: regression-guards
---

# Malformed SSE events must not disable quota enforcement — FIXED

## Conclusion

An invalid UTF-8 field line poisons its current SSE event, and the decoder drops
that event before invoking the usage visitor. Because incremental quota units
are added only inside that visitor, an upstream could poison every event and
freeze the quota accumulator indefinitely without degrading the decoder.

The decoder now counts completed poisoned events cumulatively for its lifetime.
One event is tolerated. The second event in the same response makes quota
observation unenforceable and fails closed only when all existing termination
gates hold: the client requested streaming, a quota accumulator exists, and the
resolved action is `Reject`.

## Decision

`MAX_TOLERATED_MALFORMED_SSE_EVENTS` is `1`; termination therefore occurs when
the cumulative count is greater than one.

- The unit is a completed poisoned event, not an invalid line. Several invalid
  fields in one event consume one tolerance.
- The count survives valid events and resets only with a new decoder. A
  consecutive threshold would let an upstream alternate valid and poisoned
  events forever while keeping enforcement incomplete.
- The counter saturates, adds no per-event allocation, and does not change the
  decoder's memory cap or response streaming behavior.
- `malformed()` remains sticky from the first invalid line, preserving the
  Anthropic translator's existing first-error fail-close policy. The new count
  controls only the OpenAI usage observer.

## Termination gates and precedence

Malformed handling uses the exact body-mutation gate already described by
`UsageExtractionProcessor::changes_body_length()`:

1. quota exists;
2. the client requested streaming;
3. `onExceeded` resolved to `Reject`.

On the passive OpenAI/OpenAICompatible usage-observation path, requests without
quota, non-streaming requests, and `Throttle`/`Downgrade` routes record precision
loss without this malformed-SSE termination. This tolerance does not apply to
Anthropic conversion: the translator has its own first-error fail-close rule. Current Anthropic dispatch installs only the translator, which publishes its
own usage; no separate following observer parses or estimates translated output.
See [translator-only ownership](anthropic-usage-observation-is-estimation-only.md).

When several causes become visible in one callback, the terminal cause is
chosen once in this order:

1. actual quota over-limit;
2. permanent decoder degradation;
3. repeated malformed events.

All three paths use the shared termination helper, so exactly one error event
is injected and `AiUsageShared::mark_gateway_terminated()` records an explicit gateway cut
instead of upstream truncation. Credential reconciliation still requires
reported or valid estimated usage; unknown usage retains reservations even
at EOS or gateway termination. Transport certainty alone is insufficient.

## Signals and client contract

- `AiSseDecodeMalformed` is recorded once per request after the first
  completed poisoned event, regardless of quota enforcement.
- `AiSseMalformedQuotaTerminated` records a cutoff caused by the
  cumulative malformed-event threshold.
- No metric is emitted. Request-level diagnosis stays in the access log.
- The client receives the existing boundary-safe `token_quota_unavailable` SSE
  envelope. It contains no provider bytes or malformed-input detail.

## Rejected alternatives

- **Terminate on the first malformed event:** rejects an isolated provider
  encoding fault and removes the observer's established tolerance.
- **Count invalid lines:** makes one event with several invalid fields consume
  several tolerances even though the parser's behavioral unit is the event.
- **Reset after a valid event:** creates a deterministic alternating-event
  bypass.
- **Estimate quota units for poisoned events:** invents a second incremental
  accounting policy and keeps proxying a stream whose precise output is hidden
  from enforcement.
- **Terminate without quota or under non-Reject actions:** changes established
  degradation and exceeded-action semantics and invalidates the declared body
  mutation gate.
- **Expose malformed UTF-8 to the client or add a metric:** leaks an internal
  provider detail and duplicates request-level access-log observability.

## Current accounting and verification limits

Task 04 statically checked current usage observation, translator ownership and
settlement on 2026-10-06. Decoder degradation does not automatically make the
passive usage snapshot unknown: a prior reported snapshot or available
character estimate can remain, possibly covering only a prefix. This is a
precision loss, not a guarantee of complete actual usage. Independent invalid
JSON/encoding/budget/arithmetic paths can still publish unknown usage.

The regression executions below belong to the original correction; they were
not rerun for the documentation task and do not establish new provider or
transport acceptance.

## Regression boundary

Decoder tests cover one poisoned event, several invalid lines in one event, two
events in one chunk, chunk-split invalid input, valid-event interleaving, sticky
state, and normal dispatch after corruption. Usage tests drive real malformed
bytes through the decoder and cover every gate plus over-limit/degradation
precedence and one-shot statuses.

The `EdgionBackend/SingleProvider` integration fixture emits malformed,
valid, malformed, then a delayed marker and `[DONE]`. The raw H1 test uses the
real Redis-backed Reject quota route, requires the valid middle event, parses
the final injected event as independent SSE, rejects the unreachable tail,
checks both statuses and the absence of `incomplete_response`, and finishes
before the fixture's natural deadline. Existing H2 quota-termination coverage
already exercises the protocol-specific response-pump seam; the malformed
threshold itself is transport-independent.

## WS-06 requirement

Any SSE decoder replacement in WS-06 must port this contract, not merely the
old sticky boolean: cumulative completed-event counting, one-event tolerance,
valid-event non-reset, one count per poisoned event, saturating arithmetic,
the three enforcement gates, terminal precedence, and the Redis-backed
regression must all remain intact.

## Resolution references

- `D1b` — 2026-08-11.
