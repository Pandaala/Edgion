---
knowledge_type: regression-guards
---

# Lowest-latency tests must distinguish the minimum

A `LowestLatency` selector test is not discriminating when every candidate it
compares has the same p50. Both minimum and maximum ordering select the same
set, so route weights can make a broken score direction look correct.

Selection is Power-of-Two-Choices (see
[AI adaptive selection](../../ai-routing/regression-guards/adaptive-selection-p2c-with-in-flight-load.md)),
so the worst candidate of a pair must never win. Keep a case with exactly two
mature, idle candidates at different p50 values, give the slower one a larger
route weight that does not offset its latency, and assert across an entropy
sweep that only the faster candidate wins (with two candidates every draw
compares the same pair). Separately keep a three-candidate sweep over all
ordered pairs asserting the slowest is never selected.

## Resolution references

- `test-03` — 2026-08-09.
- `gw-ai-02` — 2026-11-06: rewritten from global-argmin to P2C pair semantics.
