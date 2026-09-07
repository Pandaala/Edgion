---
knowledge_type: regression-guards
---

# Lowest-latency tests must distinguish the minimum

A `LowestLatency` selector test is not discriminating when every mature
candidate has the same p50. Both minimum and maximum aggregation select the
same tie set, so route weights can make a broken implementation look correct.

Keep a case with at least two mature candidates at different p50 values and no
bootstrap candidate. Give the slower candidate the larger route weight, then
assert across an entropy sweep that only the faster candidate wins. This makes
both the latency ordering and its precedence over route weight observable.

## Resolution references

- `test-03` — 2026-08-09.
