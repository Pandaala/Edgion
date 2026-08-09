# gw-18 — `UsageExtractionProcessor` re-buffers and re-parses the already-translated body on the Anthropic non-streaming path

| Field | Value |
|-------|-------|
| ID | `gw-18` |
| Disposition | `confirmed` |
| Severity | P3 |
| Dimension | Design-rule (bounded memory / hot-path cost) |
| Location | `edgion-gateway/src/ai/dispatch.rs:428-450`; `edgion-gateway/src/ai/usage.rs:540-578`; `edgion-gateway/src/ai/anthropic/response.rs:287-330` |
| Reviewer | Streaming / translation review |
| Status | open |

## Summary

For an Anthropic service the processor chain is `AnthropicResponseProcessor` → `UsageExtractionProcessor`. The first already accumulates the raw provider JSON (≤1 MiB), translates it into a second ≤1 MiB buffer, and publishes usage; the second then copies the translated body into a third ≤1 MiB buffer and re-parses it, even though `publish_reported_usage` is first-wins and the result is discarded.

## Details

`dispatch.rs` unconditionally pushes `UsageExtractionProcessor` into `response_processors`, including when `service.provider == AiProvider::Anthropic` — only the quota accumulator is suppressed via `usage_quota`.

`AnthropicResponseProcessor::process_non_streaming` emits the whole translated response as a single chunk at `end_of_stream`. `UsageExtractionProcessor::on_chunk` then runs `observe_chunk` → `observe_non_streaming` (a full `extend_from_slice` copy) and `finish` → `try_publish_usage` (a full `serde_json::from_slice` into a `Value` tree). `AiUsageShared::publish_reported_usage` short-circuits on `self.reported_usage.load(…)`, so the parse result is thrown away on every success. `non_streaming_chars` likewise counts characters that are only read as an estimation fallback that cannot fire.

## Impact

Peak per-request heap for a 1 MiB non-streaming Anthropic response is roughly 4× the response size — raw input, `LimitedWriter` output, usage-processor copy, and the serde `Value` tree — plus a redundant full JSON parse on the response hot path, for zero effect. Under concurrency this is straightforward memory and CPU waste.

## Suggested fix

Skip installing `UsageExtractionProcessor` when the Anthropic processor is installed — it already publishes reported usage, and the char counter for the streaming case — or give it an "observation suppressed" mode the Anthropic processor sets.

## Notes

All buffers remain individually capped, so this is waste rather than an invariant violation; the HTTP body-handling invariants themselves were verified to hold on this path.
