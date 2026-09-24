---
name: review-ai-routing
description: Review knowledge for ai-routing, grouped by decision type.
---

# ai-routing review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Regression guards

- [AI stores expose one count and key accessor per snapshot](regression-guards/ai-store-single-count-accessors.md)
- [gw-01 — CostWeighted read only `max_tokens` — FIXED](regression-guards/ai-cost-weighted-request-output-bound-parsing.md)
- [AI endpoint failure skips the service](regression-guards/ai-endpoint-failure-skips-the-service.md)
- [AI failover tests must cross runtime boundaries](regression-guards/ai-failover-tests-must-cross-runtime-boundaries.md)
- [AI Predispatch Scans Re-check End-to-End Deadlines](regression-guards/ai-predispatch-scan-rechecks-deadlines.md)
- [AI quota throttle must wait and readmit once](regression-guards/ai-quota-throttle-must-wait-and-readmit-once.md)
- [AI rule invalidity escalates to `Accepted=False` only when no rule serves — FIXED](regression-guards/ai-rule-accepted-escalates-only-when-nothing-serves.md)
- [Anthropic `pause_turn` is a successful stop](regression-guards/anthropic-pause-turn-is-a-successful-stop.md)
- [Anthropic tool choice `none` has no parallel flag](regression-guards/anthropic-tool-choice-none-has-no-parallel-flag.md)
- [Anthropic translation tests must use real wire shapes](regression-guards/anthropic-translation-tests-must-use-real-wire-shapes.md)
- [Conflicting OpenAI output bounds are client errors](regression-guards/conflicting-openai-token-bounds-are-client-errors.md)
- [Identity-model quota uses the requested logical name](regression-guards/identity-model-quota-uses-requested-logical-name.md)
- [Streaming usage is a monotonic cumulative snapshot](regression-guards/streaming-usage-is-a-monotonic-cumulative-snapshot.md)
