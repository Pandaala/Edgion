# gw-22 — SSE/body degradation flags have no production consumer, so a degraded stream silently loses provider usage

| Field | Value |
|-------|-------|
| ID | `gw-22` |
| Disposition | `confirmed` |
| Severity | P3 |
| Dimension | Observability |
| Location | `edgion-gateway/src/ai/usage.rs:189-199` (`sse_degraded`, `non_streaming_overflowed`), `:510`, `:526-540`; decoder contract in `edgion-gateway/src/sse/mod.rs` |
| Reviewer | Test-coverage review |
| Status | open |

## Summary

`UsageExtractionProcessor` uses `SseDecoder::with_default_limit()`; an event exceeding the 64 KiB cap sets a **sticky** degradation flag and stops parsing for the rest of the stream. Neither `sse_degraded()` nor `non_streaming_overflowed()` is read anywhere in production — no access-log field, no metric, no forced switch to estimated usage.

## Details

Grepping `\.sse_degraded()\|non_streaming_overflowed()` across `edgion-gateway/src` returns only the accessor definitions and their own unit tests. When degradation trips, the `usage` event the provider sends *after* the oversized event is never decoded and `reported_tokens()` stays unset — but nothing records that this happened.

The unit test `sse_overflow_degrades_without_stopping_character_count` therefore certifies a bit that changes no behaviour, and the integration case `oversized_sse_passthrough` asserts byte-preservation of the response body only: it sends no `x-trace-id`, fetches no access log, and reads no metric.

## Impact

A stream containing one oversized event silently loses provider-reported usage with no operator-visible signal: wrong `genai` token counts, wrong `estimated_cost_usd`, and on a quota'd route an under-charged quota bucket. Changing `DEFAULT_MAX_PENDING_EVENT_BYTES`, or making degradation non-sticky, breaks no test.

## Suggested fix

Wire `sse_degraded()` / `non_streaming_overflowed()` into the access-log entry (or an existing counter) so the condition is observable at all — most naturally by forcing `usage_estimated == true` when reported usage was lost to degradation. Then extend `oversized_sse_passthrough` to send a trace id and assert the resulting `genai` shape.

## Notes

Related: [`gw-06`](./gw-06-genai-model-doc-comment-vs-passthrough.md) covers the other doc-vs-reality gap on the same `genai` surface.
