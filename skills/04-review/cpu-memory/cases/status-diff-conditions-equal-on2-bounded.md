---
name: status-diff-conditions-equal-on2-bounded
description: Use when reviewing findings that flag status_diff conditions_equal or parents_status_equal as O(n²) needing HashMap; n is hard-bounded at ≤10 and HashMap overhead exceeds the linear scan cost.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `status_diff::conditions_equal` O(n²) Linear Scan on Bounded Condition Arrays is Not an Issue

**False-positive scenario**: `conditions_equal` in `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_diff.rs` uses an O(n)-find inside an O(n) loop when matching new conditions against old conditions by `type`; flagged as "O(n²), should switch to `HashMap<&str, &Value>` for O(1) lookup". Same shape applies to the parent / ancestor / listener matchers in the same file (they all match by an identity field via `iter().find(...)`).

**Reality**:

1. **Cold path**: called from `processor.rs::status_has_changed` → `status_semantically_changed` → `conditions_equal`. Triggered only on Controller status reconcile (per resource update), never per request.

2. **n is hard-bounded and tiny**: per Gateway API / Edgion conventions, condition counts are ≤ 5 for Gateway-level status and ≤ 10 across all resource kinds. Worst case ~100 `Value` field comparisons of 5 short keys each, all in-memory, no allocations. Total cost is sub-microsecond.

3. **HashMap "optimization" can regress on small n**: the suggested `HashMap<&str, &Value>` fix incurs one heap allocation for the map plus a string hash per insert/lookup. For n ≤ 10 the constant overhead is comparable to or larger than the linear scan it replaces; the change trades easily-auditable code for a likely-neutral or slower path.

4. **The module's real concern is correctness, not throughput**: `status_diff` exists to ignore `lastTransitionTime` and similar non-semantic fields so we don't issue spurious K8s API writes. The linear-scan-by-type form makes it obvious to a reviewer that the matcher key is `type` and the compared fields are `status / reason / message / observedGeneration` (see `condition_fields_equal`). A HashMap-based version moves that logic two function calls away.

**Verdict**: not a performance issue. Code-clarity rewrite suggestions are not accepted unless (a) profile data shows status diff dominating Controller CPU, or (b) the condition count bound is broken by a new resource kind that produces dozens of conditions per status. Same reasoning applies to `parents_status_equal`, `ancestors_status_equal`, and the listener loop in `gateway_status_equal` — all match by identity (parentRef / ancestorRef / listener name) on the same bounded-cardinality input.
