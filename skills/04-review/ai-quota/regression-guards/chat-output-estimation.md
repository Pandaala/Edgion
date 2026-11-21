---
name: chat-output-estimation
description: Use when reviewing fallback output-token estimates for native or translated OpenAI Chat responses.
knowledge_type: regression-guards
---

# Chat output estimation and missing unary usage

## Conclusion and scope

When a Chat stream has no reported usage and supports estimation, sum the decoded
Unicode scalar counts for generated text, reasoning content and tool arguments
across the attempt. Round up once after dividing by four. Splitting the same text
across complete events or changing the model ID leaves the estimate invariant.
For a successful non-streaming Chat JSON document without provider usage, publish
`Unknown` with absent numeric usage and cost. The bounded unary observer validates
and discards content strings while forwarding the document.

## Rationale and evidence

Originally `UsageExtractionProcessor` counted SSE data-field values or the whole
JSON document, and the Anthropic translator counted re-encoded Chat chunks.
Repeating IDs, object keys and empty deltas increased `output_tokens` even when
the provider generated no additional content. Those estimates flow through
`AiUsageShared::accounting_snapshot` into quota, estimated cost and usage-based
routing. The streaming paths now measure the fields already decoded by
`observe_canonical_block` or by the translator handling provider deltas.
Provider-reported usage retains priority. The streaming enforcement accumulator
continues to track output units separately from the estimate.

## Verification and re-evaluation

`cargo test -p edgion-gateway --lib 'ai::'` passed all 619 selected tests
on 2026-11-21, including the access-log finalization and partial-stream cases.

Check the gateway unit suites for AI usage extraction and Anthropic response
translation when modifying the decoded fields, partial-stream settlement or
fallback eligibility. Cover escaped scalars, choice and tool deltas, event
boundaries, metadata-only events and upstream usage precedence. A valid observed
content prefix can support an estimate on an interrupted stream. An incomplete
or unreadable event with no measured content leaves fallback usage unknown on
both native and translated Chat streams. Complete metadata-only events add zero.
The translator's
[termination gate](gateway-terminated-stream-settles-its-reservation.md)
can exclude the last callback. Revisit unary estimation if the bounded observer
can count generated fields incrementally without retaining the response.

## Resolution references

- `gw-ai3-02` — corrected output estimation for missing provider usage.
