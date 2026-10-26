# Decide streamed Anthropic conversion event input representation

Status: deferred representation and performance decision for user review.
Source baseline: `5616ebf68`. No runtime implementation selected.

## Current evidence

`AnthropicResponseProcessor::process_sse_event` in
`edgion-gateway/src/ai/conversion/chat_to_anthropic/response.rs` obtains canonical
SSE semantic data, parses a complete owned serde_json Value, requires an object
and string type, and checks the SSE event name before dispatch. Even ping and
unknown event types pass these checks; unused members become part of the DOM.
The redundant known-event classifier has already been removed.

Handlers borrow fields for synchronous validation and translation, but the
processor retains message_id across callbacks. That identifier must remain
owned unless a separately reviewed owner supplies an equally valid lifetime.
Replacing every String with a borrow is not a mechanical optimization.

The canonical decoder bounds pending event bytes (default 1 MiB), ranges, lines
and metadata. These bounds do not establish an exact DOM heap bound.
SseEventRef::semantic_data can borrow a contiguous range or materialize fragmented
or multiline data. Removing a Value tree would not automatically remove that
semantic-data buffer, output trees or retained processor state.

The locked serde_json Value deserializer inserts ordinary object keys into a
map, so repeated keys replace previous values, including entire nested parents.
Derived structs, skipped values and selective parsers can differ in duplicate,
numeric, recursion and malformed-input acceptance. The enabled raw_value feature
also has special key classification in Value deserialization; ordinary last-wins
behavior is not a universal characterization of every possible input.

## Decision and scope

Compare retaining the current parser with a selective or borrowed per-event
representation using existing canonical mechanisms. First establish exact
semantic and budget equivalence; availability of JsonStreamSelective does not
prove compatibility with the current serde_json Value contract. If a behavior
change is needed, expose it for a separate decision rather than silently relaxing
validation or limits. No alternative is selected and no performance gain is proven.

This is separate from [request tree ownership](conversion-01-request-tree-ownership.md),
[output envelope ownership](sse-03-canonical-event-envelope.md), and
[native selective-observer fragment feeding](body-04-selective-observer-input.md).
Do not expand it into unary conversion, request parsing, a second SSE decoder,
response BodyStorage, full-response aggregation, another cache or new observability.

## Acceptance for a selected implementation

- Freeze exact source, mirrored unit-test and knowledge paths under independent
  IMPL plan and final review. Preserve canonical framing, semantic-data joining,
  poisoned/malformed decoder policy, event/type equality and terminal checks.
  Unknown events and ping must not bypass complete JSON validation.
- Differentially compare complete-document acceptance, trailing bytes, malformed
  escapes, escaped field names/types, numeric extremes, recursion boundaries,
  selected and unknown duplicate keys, wrong-last-type values and nested parent
  replacement. Include feature-dependent serde behavior. Do not assume a typed
  visitor or ignored-value parser matches Value validation.
- Preserve all eight known event paths and unknown-event handling, block/tool
  indexes, active-block transitions, message start/stop, late events and EOS.
  Preserve cumulative usage, input/output-seen flags, quota update ordering,
  fixed redacted errors, stream sealing, exact output bytes and semantic ranges.
- Exercise contiguous, every-split and multiline events, empty/control events,
  unknown large subtrees, invalid unknown members and exact size/depth/work caps.
  Keep failure, cancellation and finalization ownership unchanged. Borrowed
  event data may not escape its callback or survive an await; retained identity
  remains owned. Native passive observers retain their separate failure policies.
- Measure allocation calls, copied bytes, peak and retained memory, and CPU for
  frequent small events, large text/tool fragments, escaped strings and large
  unused subtrees. Account separately for semantic-data merging, parser storage,
  output construction and retained state. A byte cap is not a heap budget and
  source-level borrowing is not a throughput or allocation-free guarantee.

## Baseline verification

Fresh unchanged-source tests passed with these exact filters:
`ai::conversion::chat_to_anthropic::response::tests` (32), `sse::tests` (50,
including matching nested module scopes), and `ai::guards::semantic::response::tests`
(25): 107 total. These tests check the current implementation; they do not prove
parser equivalence or performance improvement. No prototype, runtime change,
new test, full suite or live-provider integration was introduced.

Original knowledge: `skills/04-review/ai-routing/regression-guards/anthropic-translation-tests-must-use-real-wire-shapes.md`
and `skills/04-review/sse/regression-guards/canonical-chat-block-classification.md`.
The latter describes canonical native Chat observation, not permission to relax
the stricter conversion event contract.
