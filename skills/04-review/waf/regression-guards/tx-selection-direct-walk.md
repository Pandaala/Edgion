---
name: tx-selection-direct-walk
description: Use when reviewing native WAF value selection for TX literal or regex targets, selection order, metered TX key reads, or the select_values cursor path.
knowledge_type: regression-guards
---

# TX literal and regex targets walk TX entries directly

## Conclusion and scope

`select_values` (`edgion-waf/src/seclang/transaction/selection.rs`) obtains
each candidate that passes a target's selector from `next_selected`. For the
`TX` collection with a `Literal` or `Regex` selector, `next_selected` walks
`TxState::entries` directly. All other collections and selectors (`None`,
`Path`) still go through `CandidateCursor::next` and `selector_matches`.

The direct walk must stay observably identical to the generic path:

- the same entries in the same order (index `0..entries.len()`, re-read each
  step, one candidate per matching entry, no early stop after a hit);
- the same metered key read per entry (`MatchValue::bytes` with the JSON
  inspection budget), in the same order as `selector_matches`:
  `generation_bytes` before the key read for literals; key read, program
  lookup, then scratch slot for regexes;
- ASCII case-insensitive literal comparison (the generic `TX` rule);
- `ctl` target exclusions, `SelectedValueLimit`, retained-storage admission and
  `&TX` counts are applied by the unchanged caller loop in `select_values`, so
  the `None` and `Count` modifiers share the walk. The `Exclude` modifier still
  filters already-selected values through `selector_matches`.

The walk allocates nothing and adds no index, cache, pool, lock or
thread-local. A `#[cfg(test)]` switch (`Transaction::force_generic_selection`,
`SelectionCtx::generic_selection`) forces the generic path as an oracle.

## Rationale and evidence

The 2026-11-07 second-pass profile (plan T12c, recorded in
`docs/superpowers/plans/2026-11-07-waf-second-pass-baseline.md`, "Profile")
measured `select_values` at 19.3 % of clean_get after T12a/T12b. Counting
selector calls showed about 5 900 per clean_get request, of which about 4 400
were `TX:<literal>` comparisons (84 targets each re-scanning about 52 TX
entries) and about 380 were `TX:/regex/` matches. Each entry paid a
non-inlined `CandidateCursor::next` returning `Result<Option<SelectedValue>>`,
then `selector_matches` and `selected_name_bytes` dispatch. The direct walk
removes that per-entry overhead; it does not remove the comparisons.

## Alternatives and limits

- A per-request lowercase index of TX names was rejected: TX changes between
  units (and is staged in place, see
  [tx-unit-staging-in-place](tx-unit-staging-in-place.md)), so an index would
  need maintenance on every `setvar` and rollback.
- Stopping a literal scan at the first hit (TX keys are unique
  case-insensitively) was rejected: it would skip later metered key reads and
  change JSON-budget consumption.
- Memoizing regex selector results per entry was not attempted; the regex cost
  itself remains.

## Verification and re-evaluation

Unit test
`direct_tx_selection_matches_the_generic_cursor_order_counts_and_exclusions`
(`edgion-waf/tests/unit/seclang/transaction/mod_test.rs`) compares the direct
and generic paths (findings, full TX snapshot, `MATCHED_VARS` order, ctl and
unit work counters). It covers mixed-case TX literals, TX regexes with `!TX:`
exclusions, `&TX` counts, mid-phase `ctl:ruleRemoveTargetByTag`, duplicate and
mixed-case headers, header regex selectors and `ARGS_GET` exclusion/count.
Re-evaluate if `CandidateCursor` gains TX-specific behavior, TX keys stop being
unique, or `MatchValue::bytes` changes its metering.

## Resolution references

- Plan T12c (P3 selection), 2026-11-07; commit `perf(waf): walk TX entries directly for TX literal and regex targets in value selection`.
