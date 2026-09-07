---
name: conditions-hashset-wildcard-bucket-split
description: SUPERSEDED. Include/Exclude and wildcard_patterns were deleted in condition-layer S7/S9 (D-7). Re-reports of the bucket-split / Include-Exclude duplication are not accepted. Exact KeyMatch values still use a HashSet above the threshold.
knowledge_type: superseded
---

> **SUPERSEDED by condition-layer S7 / S9 (D-7).** `IncludeCondition`,
> `ExcludeCondition`, and `wildcard_patterns` no longer exist. Wildcard
> `values` (`prefix*`) are now regex. Exact values still use a HashSet
> above the threshold on `KeyMatch`. Re-reports of the bucket-split /
> Include-Exclude duplication are **not accepted** because those types
> no longer exist. The closed-fix text below is a historical record of
> the 2026-05-27 shape only.
>
> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.
> Current layer: [plugin-metadata/unified-condition-layer.md](../../plugin-metadata/superseded/unified-condition-layer.md).

# `IncludeCondition` / `ExcludeCondition` HashSet/Wildcard Bucket Split (plugins-runtime-F005)

## Historical record (2026-05-27 closed-fix shape)

The types and fields named below (`IncludeCondition`, `ExcludeCondition`,
`wildcard_patterns`) were deleted in condition-layer S7/S9. Do not treat
this section as the current matcher.

**Closed-fix shape (historical).** `IncludeCondition::compile()` / `ExcludeCondition::compile()` in `gateway/plugins/runtime/conditions/types.rs` populate `values_set` (exact entries) and `wildcard_patterns` (entries containing `*`) **together**, gated by `exact_values.len() > VALUES_HASHSET_THRESHOLD`. When the gate fires, `wildcard_patterns` is set to `Some(_)` **even when the wildcard list is empty**. `matches_value` in `gateway/plugins/runtime/conditions/evaluator.rs` then walks:

1. `if let Some(set) = &self.values_set { if set.contains(value) { return true; } }` — O(1) exact-match probe.
2. `if let Some(patterns) = &self.wildcard_patterns { … } else if let Some(patterns) = &self.values { … }` — `else if` is load-bearing: when the bucket split ran, `self.values` is dead from the matcher's point of view; only the (typically tiny) wildcard subset is iterated. When the split did **not** run (≤ threshold exact entries), the matcher falls back to scanning the full small `self.values` directly — same shape as before the optimisation, no behavioural regression on the small-list path.
3. Compiled regex via `compiled_regex.is_match(value)`.

The "always `Some(_)`, never `None` when split ran" choice is the subtle bit. For the canonical W=0 case (large all-exact allowlist / denylist, no wildcards at all), `wildcard_patterns = Some(vec![])` keeps the `else if` arm dead. If `compile()` set `wildcard_patterns = None` when wildcards were empty, the matcher's `else if let Some(patterns) = &self.values` would re-enter the original O(N) rescan over every exact entry already covered by `values_set` — exactly the F-005 defect under a different name.

**Why this matters (historical).** Plugin-runtime conditions sit on the request hot path: every rate-limit, auth-guard, region-route, bandwidth-limit, etc. with a configured `include` / `exclude` block calls `matches_value` per request. With > 16 exact values, the pre-fix shape paid a full O(N) Vec scan on every non-matching request despite the HashSet's O(1) probe succeeding (as a miss).

**Re-report patterns that are not accepted (historical closed-fix; the types are gone):**

- "`matches_value` rescans exact entries on miss / HashSet provides no benefit on miss path" — closed by the bucket split + `else if`.
- "compile builds `values_set` but `self.values` still contains exact entries — wasted memory / redundant work" — `self.values` is kept on purpose so the ≤-threshold fallback path stays correct; the matcher's `else if` guarantees it is not walked when the split ran. Re-trimming `self.values` in place is an unnecessary mutation that does not change the eval-time behaviour.
- "compile sets `wildcard_patterns = None` when no wildcards exist, falling back to `self.values`" — would re-introduce F-005; verify `compile()` always assigns `Some(_)` (possibly empty) when `values_set` is populated.

**Re-open criteria (historical; do not re-open for Include/Exclude or wildcards).** Re-evaluate the bucket-split shape only if (a) a third match dimension is added beyond "exact + wildcard + regex" (e.g. a new prefix/suffix-tree fast path), (b) `VALUES_HASHSET_THRESHOLD` is being tuned and someone wants to consider mutating `self.values` in place as a memory optimisation, or (c) the `Option<Vec<_>>` field shape becomes a profile-visible cost at very high condition counts. After S7, (a) cannot fire for wildcards — they are regex.

**Reference points (verified 2026-05-27; paths and types are historical):**

- `gateway/plugins/runtime/conditions/types.rs` — `pub wildcard_patterns: Option<Vec<String>>` on both `IncludeCondition` and `ExcludeCondition`, populated by `compile()`.
- `gateway/plugins/runtime/conditions/evaluator.rs` — `if let Some(patterns) = &self.wildcard_patterns { … } else if let Some(patterns) = &self.values { … }` in both `IncludeCondition::matches_value` and `ExcludeCondition::matches_value`.
- `KeyMatchCondition::compile()` does **not** carry a wildcard bucket (its `matches_value` already uses `else if` correctly and has no wildcard semantics on `values`). After S7 this is the only remaining HashSet path: exact `KeyMatch` values above the threshold.

Companion fixes that share the "compile-time pre-split for a hot-path matcher" pattern: [http-grpc-route-regex-matching-no-fallback.md](../review-evidence/http-grpc-route-regex-matching-no-fallback.md) (route regex), [hashmap-new-default-capacity-reconcile-paths.md](../review-evidence/hashmap-new-default-capacity-reconcile-paths.md) (the opposite verdict: when the path is not hot, pre-split / pre-allocate is not warranted).

## Historical resolution evidence: plugins-runtime-F004

Superseded by condition-layer S9: Include/Exclude deleted; re-reports of Include/Exclude duplication are not accepted. Historical: `IncludeCondition::matches_value` and `ExcludeCondition::matches_value` (`gateway/plugins/runtime/conditions/evaluator.rs`) were byte-identical ~40-line bodies — HashSet exact-match probe → `wildcard_patterns` bucket / `values` fallback split (F-005 dedup) → compiled regex match with `"regex not compiled"` PluginLog fallback. The only behavioural difference between Include and Exclude lives in the `evaluate*` wrappers (Include returns as-is, Exclude negates with `!`); every F-005-class logic change had to land in both impls in lock-step. Consolidated by extracting a module-private `#[inline] fn matches_pattern_set(value, values_set: Option<&HashSet<String>>, wildcard_patterns: Option<&[String]>, values: Option<&[String]>, compiled_regex: Option<&regex::Regex>, regex: Option<&[String]>, log: Option<&mut PluginLog>) -> bool`; both struct `matches_value` methods are now 9-line wrappers that forward references via `.as_ref()` / `.as_deref()`. The F-005 `else if` between the wildcard and values arms is preserved verbatim — substituting two independent `if`s would re-introduce the exact-match double-scan defect on the hot-path miss case. Pure refactor — data-plane behaviour bit-identical, no `#[serde]` field changes, no CRD schema change, no test struct-literal churn (the existing `test_include_path` / `test_exclude_path` cover both paths through the helper). The trait-with-default-method alternative (private trait + 5 accessors per struct) was rejected — same dedup benefit but ~10 trivial accessor methods + 1 trait surface vs. 2 thin wrappers + 1 free fn, and trait dispatch is one extra layer the compiler must inline through on the hot path. Re-reports of "IncludeCondition and ExcludeCondition matches_value duplicated" / "extract a private trait for the shared pattern-set lookup" are not accepted; future logic changes to the pattern-set lookup must land in `matches_pattern_set` and not be re-inlined into the struct impls.

## Resolution references

- `plugins-runtime-F004` — 2026-05-27 (superseded by condition-layer S9).
- `plugins-runtime-F005` — 2026-05-27 (superseded by condition-layer S7).
