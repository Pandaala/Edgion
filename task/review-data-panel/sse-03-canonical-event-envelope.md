# Canonical streaming event envelope ownership

Status: deferred representation/API decision for user review. No runtime change.
Source baseline: `865f64796`.

## Evidence

`AnthropicResponseProcessor::push_chunk` moves its completed choice into a
one-element choices Vec, but still constructs an outer JSON Map, five owned keys,
the fixed object-name String and copies of message_id and canonical_model for
each output event. `handle_message_stop` optionally constructs the same identity
envelope plus an empty choices array and usage object. The prior owned-choice
optimization removed recursive choice reconstruction, not these envelope owners.
Empty strings need not allocate; no universal allocation count is asserted.

The processor receives the canonical model and created timestamp at construction.
The provider message ID becomes available only after validated message_start;
it cannot be prepared at configuration publication. These identity fields are
retained for the stream, while choice content and final usage change per event.

`write_canonical_record` accepts &Value. With capture disabled it uses its bounded
serde writer; with capture enabled it walks the structure and records exact JSON
string token ranges for content, reasoning and tool arguments in the same pass.
Generated output is not decoded again or searched for matching string values.
The non-semantic-container fast path does not remove the outer Value ownership.
A borrowed Serialize struct alone does not supply the structural capture contract.

`on_chunk` selects semantic capture from the session for each callback, then
publishes or reclaims the resulting batch according to terminal ownership.
`push_json_event` also enforces aggregate output bounds and counts encoded JSON
characters. Envelope representation must not silently change those observations.

## Decision alternatives

- Retain the current Value representation if its cost does not justify another
  canonical serialization interface.
- Introduce a borrowed event view through the existing canonical writer, with
  an explicit way to preserve semantic-path classification and ranges. Any
  separate plain/capture implementation needs byte and behavior parity proof;
  do not duplicate the SSE encoder or recover ranges by searching/reparsing bytes.
- Prepare bounded, processor-owned serialized identity fields after their inputs
  are available. Define lifetime, escaping, field order, offset adjustment and
  memory cost, including simultaneous prepared and output storage. This is derived
  state owned by one processor, not a new cache or response-body buffer. Do not
  assume a contiguous prefix fits the existing JSON key order or freeze the
  callback's semantic-capture decision at preparation time.

No alternative is selected. Extending this to request conversion, native sources,
unary response conversion or arbitrary plugin JSON is outside the task. Do not
introduce response BodyStorage, pooling, unbounded retention, a second semantic
window or new observability to implement the optimization.

## Acceptance before implementation

Preserve exact SSE framing, JSON field order and scalar encoding, escaping,
choice/tool indexes, semantic token ranges and unavailable semantics. Exercise
repeated identical text in identity and semantic fields so range correctness
cannot depend on string searching. Cover text, tool-start/argument/finish events,
terminal usage, DONE, empty values, escaped and multibyte identities, and both
capture modes across callback boundaries.

Keep inclusive event caps, checked aggregate-output and batch admission, metadata
limits, complete-record publication, output-character accounting, quota updates,
guard handoff and terminal precedence. Reject insufficient output space without
publishing partial events. Verify failure, cancellation and retained-window EOS
cleanup; no callback-borrowed input may survive an await.

Measure small/high-count and larger events, tool-heavy streams, terminal usage,
short and escaped identities, with capture enabled and disabled. Compare copied
bytes, allocation calls, retained capacities, CPU and concurrent memory costs.
Source ownership evidence alone proves no throughput or RSS improvement. Freeze
the implementation allow-list and obtain independent IMPL plan/final review.

## Original knowledge and verification

- [Canonical SSE substrate](../../skills/01-architecture/02-gateway/00-overview.md#canonical-sse-substrate)
- [Typed canonical classification](../../skills/04-review/sse/regression-guards/canonical-chat-block-classification.md)
- [Window capacity admission](../../skills/04-review/sse/regression-guards/window-capacity-admission.md)
- [Translation wire fixtures](../../skills/04-review/ai-routing/regression-guards/anthropic-translation-tests-must-use-real-wire-shapes.md)

Fresh unchanged-source verification passed 14 canonical SSE, 62 converter and
25 semantic response tests (101 total). These verify existing behavior, not a new
borrowed view or prepared-field design. No prototype, benchmark, full suite or
live integration ran for this audit.
