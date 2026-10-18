# Selective SSE observer input representation

Status: deferred optimization for user decision. No runtime change.
Source baseline: 3ab7257cf.

## Evidence

Native Anthropic MessagesProcessor and OpenAI Responses observe_event obtain
SseEventRef::semantic_data(), then feed the resulting contiguous bytes to their
selective JSON observers. A single contiguous data range can borrow; a range
spanning retained slices or multiple data lines may allocate a merged String.
Multiple data lines insert exactly one newline between semantic values.

Both observers use edgion-waf's canonical JsonStreamSelective parser. Its
JsonStreamLimits::max_work explicitly accounts for physical work that depends
on input segmentation. In json_stream.rs, borrow_string_prefix can deliver a
complete token from the current input but retains a prefix when the input ends.
Replacing one contiguous feed with several fragment feeds therefore changes
where token retention, copies and growth occur. The net allocation, work and
failure-boundary effect under production limits has not been measured or proven.
This audit does not establish a current regression or guaranteed total-memory gain.

## Decision

Choose whether to keep materialized semantic input or investigate a canonical
fragment-feeding API for these two selective observers. Avoiding an event-sized
merge may help fragmented events, but input segmentation must not silently change
accepted metadata or resource-limit behavior. Do not treat an incremental parser
API alone as proof of equivalence. Contiguous event input already borrows.

This proposal excludes Chat-to-Anthropic conversion and the offset-based canonical
Chat parser. It also does not authorize parser pooling, new response storage or
changes to lexical budgets in the shared leaf crate.

## Acceptance for a selected implementation

- Reuse canonical SSE ownership for semantic fragment traversal; do not implement
  another event decoder, line joiner or response-body cache in either consumer.
- Differentially compare current materialized input with proposed input at all
  split positions, including UTF-8 scalars, escapes, selected and discarded
  strings, numbers, duplicate keys, empty data and multiple data lines.
  Preserve inserted newlines and each consumer's poisoned/data eligibility policy.
- Compare completed observer results, invalid/missing distinctions, selected model
  attribution, native sticky unknown, error outcome, sequence tracking and quota
  decisions. Finish exactly once with identical document boundaries; no partial
  metadata may become authoritative merely because a fragment was consumed.
- Verify source, raw-token, decoded, depth, event, retained-capacity and work limits
  at exact boundaries and after failures. If equivalence under current production
  limits cannot be established, present the exact changed acceptance behavior for
  a separate decision. Do not relax any limit as an optimization workaround.
- Measure total allocations, copied bytes, retained capacity and CPU on contiguous,
  fragmented and multiline events. Separate removed merge buffers from newly
  retained token fragments; report both improvements and regressions.
- Preserve streaming ownership, backpressure, cancellation, finalization and all
  existing diagnostic/termination rules. No full-response aggregation or new
  hot-path logging/metrics is permitted.
- Freeze an exact allow-list under a new IMPL plan and obtain independent review
  before changing either consumer or the shared SSE API.

Sources: edgion-gateway/src/sse/mod.rs, ai/protocols/anthropic/processor.rs,
ai/protocols/anthropic/usage.rs, ai/protocols/openai/responses.rs and
ai/protocols/openai/json_usage.rs; edgion-waf/src/seclang/json_stream.rs.
Native Messages/Responses accounting and canonical SSE review knowledge remain
applicable. Existing tests are baseline evidence, not proof of the proposed feed
representation or production-budget equivalence.
