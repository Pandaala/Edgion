# Evaluate JSON grammar-stack storage for SSE usage observation

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `0227a9f49`. No parser or memory-policy change.

## Evidence

Native Messages response processing constructs `Observer::for_envelope` for
recognized Data records in `ai/protocols/anthropic/processor.rs`. Native Responses
`ai/protocols/openai/responses.rs::observe_event` constructs a fresh
`JsonUsageObserver::responses(true)`. These Gateway observers share the canonical
`edgion-waf/src/seclang/json_stream.rs` grammar owner. Construction precedes
semantic-data extraction; malformed/empty input need not reach frame allocation.
The non-streaming observers instead span body chunks. Do not describe all of
this as one allocation per response chunk.

The canonical parser starts with an empty `Vec<Frame>`. `push_frame` grows it
fallibly as containers open, charges growth work and checks actual retained
capacity. Even a root object/array needs stack capacity. Completion or failure
calls `release`, which replaces the stack with an empty Vec; abandonment follows
the same failure-release path. Repeated valid SSE documents can consequently
repeat grammar-stack allocation/growth. This is source evidence, not an allocator
profile or proof of a significant end-to-end cost.

## Decision and alternatives

Measure observed event sizes/depths, frame capacities, allocation calls/requested
bytes, growth-copy work, fixed parser layout, live/peak memory and full callback
CPU before choosing a representation. Include shallow and deep, mixed valid and
invalid documents, small chunks, many events and long-lived streams. Retaining
the current representation is a valid outcome.

A small inline frame stack may avoid shallow allocations but changes the parser's
fixed footprint and its admission behavior. Existing fixed inline token storage
is intentionally excluded from retained heap accounting. Extending that pattern
to grammar frames could admit a root container at `max_retained_bytes = 0`, where
the current Vec fails. Do not call that outcome equivalent or disguise fixed
inline bytes as heap usage. Capacity growth also contributes to work counters.

Reusing owned grammar scratch between documents may reduce repeated allocation,
but merely reusing the Observer value does not help while `release` frees its
stack. Retaining capacity requires a reviewed reset/release and accounting
contract, including the lifetime between events and after failure/cancellation.
No global buffer pool, provider-specific JSON grammar, response BodyStorage or
full-response aggregation is authorized by this task.

## Acceptance boundary

Preserve current errors, partial charges, retained/peak/work accounting and
terminal release unless the user explicitly approves a separately reviewed
admission/lifecycle redesign. Any revised contract must specify physical storage,
allocation failures, growth overlap, bounded retention and observable budget
changes; lower allocation counts alone are insufficient justification.

An independent document must never inherit grammar frames, pending keys, partial
UTF-8, selected strings, sink metadata, provisional usage or terminal status from
its predecessor. Keep malformed/abandoned documents from publishing totals and
preserve actual EOS, native lifecycle, duplicate selection and quota semantics.
Exercise whole/byte/mixed feeds, independent and competing budgets, stack spills,
malformed input, successful completion, error, abandonment and repeated terminal
calls. Verify both ordinary WAF and selective Gateway consumers of the shared
owner; a provider-local benchmark cannot establish canonical-parser equivalence.

Original sources:
`skills/04-review/waf/regression-guards/fused-json-inspection.md`,
`skills/04-review/ai-routing/regression-guards/native-messages-preservation-and-accounting.md`
and `skills/04-review/ai-routing/regression-guards/native-responses-observation-and-continuation.md`.
This concerns grammar frames, not selected token storage: the user-declined token
growth hint in `skills/04-review/waf/intentional-design/json-token-geometric-growth.md`
remains excluded. body-04 owns semantic fragment input; usage-03 owns iteration
model strings. Any implementation needs a separate IMPL plan, exact cross-crate
allow-list and independent review.

Fresh unchanged-code verification passed 36 canonical parser and 80 Gateway
protocol tests (116 total), formatting, workspace all-target check/Clippy and
agent-doc validation. Existing unrelated warnings remain. These tests validate
the current representation, not any proposed redesign. No new tests, full suite,
live integration or performance benchmark ran.
