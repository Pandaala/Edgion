---
name: review-ai-quota
description: Review knowledge for ai-quota, grouped by decision type.
---

# ai-quota review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Regression guards

- [Shared cumulative buckets preserve active-marker retention](regression-guards/shared-cumulative-bucket-retention.md)
- [Anthropic translation owns usage and fallback estimation](regression-guards/anthropic-usage-observation-is-estimation-only.md)
- [A gateway-terminated AI stream settles its credential reservation and is not an upstream truncation — FIXED](regression-guards/gateway-terminated-stream-settles-its-reservation.md)
- [A definitive provider rejection settles as zero consumption — FIXED](regression-guards/provider-rejection-settles-as-zero-consumption.md)
- [A gateway-injected SSE error event must carry its own event boundary — FIXED](regression-guards/injected-error-event-needs-its-own-sse-boundary.md)
- [SSE decoder degradation must not silently disable mid-stream quota enforcement — FIXED](regression-guards/sse-degradation-must-not-disable-enforcement.md)
- [Malformed SSE events must not disable quota enforcement — FIXED](regression-guards/sse-malformed-events-must-not-disable-enforcement.md)
- [Chat output estimation and missing unary usage](regression-guards/chat-output-estimation.md)

## Superseded

- [Retired non-streaming character counter](superseded/non-streaming-char-counter-ascii-fast-path.md)
