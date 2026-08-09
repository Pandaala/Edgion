# gw-20 — `SseDecoder` dispatches an event for an empty `data:` field, contrary to the WHATWG algorithm

| Field | Value |
|-------|-------|
| ID | `gw-20` |
| Disposition | `confirmed` |
| Severity | P3 |
| Dimension | Correctness |
| Location | `edgion-gateway/src/sse/mod.rs:248-271`; consequence at `edgion-gateway/src/ai/anthropic/response.rs:370-382` |
| Reviewer | Streaming / translation review |
| Status | open |

## Summary

A lone `data:` (or `data: `) line sets `cur_has_data = true` with an empty `cur_data`, so the decoder dispatches an event whose `data` is `""`. The WHATWG EventSource algorithm explicitly discards that case.

## Details

`consume_line` does `self.cur_data.push_str(value); self.cur_has_data = true;` with no emptiness check, and `dispatch` fires whenever `cur_has_data && !cur_poisoned`. The HTML spec's "dispatch the event" step says: "If the data buffer is an empty string, set the data buffer and the event type buffer to the empty string and return" — no event.

In `AnthropicResponseProcessor::process_sse_event` the resulting empty `event.data` hits `serde_json::from_str("")`, which returns `Err`, setting `failed = true` and driving `fail_streaming` — the client gets the fixed `response_translation_failed` event and the rest of the stream is dropped. In `UsageExtractionProcessor` the same event is harmless: the parse failure is ignored.

## Impact

An upstream that uses a bare `data:` line as a heartbeat — legal SSE that a spec-conformant consumer ignores — kills the whole translated response. Anthropic itself uses `event: ping`, so this only bites an Anthropic-protocol-compatible endpoint (self-hosted proxy, another vendor's compatible surface) with a different heartbeat convention.

## Suggested fix

In `dispatch`, treat `cur_has_data && cur_data.is_empty()` as "no event" — reset and return — matching the spec. Add a decoder unit test for a bare `data:` line between two real events.

## Notes

The rest of the SSE framing was checked and is solid: BOM and CR/LF splits across chunks, CRLF byte accounting, multi-byte UTF-8 split mid-codepoint, comment lines, multi-line `data:` joining, the pending-byte cap invariant, and sticky degradation all behave correctly.
