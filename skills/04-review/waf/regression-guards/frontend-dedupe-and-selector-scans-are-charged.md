---
name: frontend-dedupe-and-selector-scans-are-charged
description: Use when reviewing WAF rule-bundle member/identity deduplication, bundle include and phrase-asset lookups, exception id-selector resolution, or the regex repetition-work estimate.
knowledge_type: regression-guards
---

# Front-end dedupe, selector scans and nested quantifiers are charged before work

## Conclusion and scope

Operator-supplied rule sets are a compile-time denial-of-service surface. Three
front-end scans charge their work before performing it; valid rule sets keep
their acceptance unchanged.

- **Bundle preparation** (`edgion-waf/src/seclang/include.rs`,
  `prepare_rule_bundles`): the total member count across all bundles is checked
  against `IncludeLimits::max_sources` before any member is validated or
  compared. Every bundle-identity, entrypoint and member-path equality goes
  through `PathComparisonBudget`, which charges an equal-length pair its full
  length (`str` equality compares lengths first, so unequal lengths cost no byte
  work) against `IncludeLimits::max_identity_comparison_bytes` and fails with
  `IncludeError::IdentityComparisonLimit` before comparing. The same budget
  continues into the `BundleIncludes` resolver lookups; `BundleAssets` lookups
  get their own budget of the same size. A resolver that exhausts the budget
  returns a generic provider failure, which the caller maps back to
  `IdentityComparisonLimit`. The include expander keeps its own stack-identity
  counter under the same limit.
- **Exception union** (`edgion-waf/src/seclang/semantic/validate.rs`,
  `resolve_exception_union`): a selector's lookup work is charged and checked
  before any matched index entry is read, and each entry's unit of work is
  charged and checked before its forward-location test. There is no uncharged
  pre-scan. Total charges per entry are unchanged, so accepted inputs report the
  same counters.
- **Regex repetition work** (`edgion-waf/src/matcher.rs`,
  `HirMetricsVisitor`): each finished subtree carries an expanded weight instead
  of a node count. A repetition's work is still `min * child + (max - min) *
  child` (or `+ child` when unbounded), but `child` is now the child's expanded
  weight, and the repetition pushes `copies * child + 1` where `copies` is `max`
  for `x{n,m}` and `max(n, 1)` for `x{n,}`. Nested counted repetitions therefore
  compose multiplicatively. All arithmetic saturates, so an overflowing estimate
  is a `RepetitionWorkLimit` rejection, not an arithmetic error. Regexes without
  nested counted repetition keep their previous estimate.

## Rationale and evidence

Second-pass review S1 (2026-11-07) found three uncharged scans:

- F-S1-1: nested linear `any` scans over up to 4 KiB paths made member and
  identity dedupe O(M^2) path compares with no charge, and the member-count
  guard ran after the loop; the resolver `find` scans were also uncharged.
- F-S1-2: the forward-id-selector check iterated every matched rule of a
  selector before the first `selector_work` charge.
- F-S1-3: `(a{1000}){1000}` scored about 3000 repetition work while unrolling to
  about one million NFA states; only regex-automata's incremental
  `nfa_size_limit` and the later scratch-byte check stopped it, after the NFA
  was built.

## Alternatives and limits

A hashed dedupe was considered for bundles; charged linear scans were kept
because the up-front member-count check bounds the scan count and they keep the
existing error precedence among `Identity`, `Entrypoints`, `MissingEntrypoint`,
`Path` and `DuplicateMember`. The repetition weight is an estimate, not an exact
NFA state count; `nfa_size_limit` remains the allocation-bounded second line of
defense. No calibration of `max_regex_repetition_work` (default 1,000,000) was
needed: every supported CRS regex still compiles.

## Verification and re-evaluation

- `cargo test -p edgion-waf`: `bundle_member_count_is_checked_before_member_validation`,
  `bundle_member_dedupe_comparisons_are_charged`,
  `bundle_include_lookups_are_charged`, `bundle_asset_lookups_are_charged`,
  `bundle_at_member_cap_with_short_paths_compiles_under_default_limits`
  (`tests/unit/seclang/include_test.rs`);
  `forward_selector_scan_is_charged_before_iteration`,
  `forward_selector_is_still_rejected_under_default_limits`
  (`tests/unit/seclang/semantic/mod_test.rs`);
  `nested_counted_repetition_work_is_multiplicative`,
  `nested_counted_repetition_is_rejected_before_the_nfa_build`,
  `single_level_counted_repetition_within_the_limit_still_compiles`
  (`tests/unit/matcher_test.rs`).
- `cargo test -p edgion-resources` (supported CRS bundles compile).
- FTW inventory against CRS v4.25.1 is unchanged:
  `active_rules=567 admitted=4741 passed=4741 mismatched=0 ... negative_admitted=896 negative_mismatched=0`.

Re-evaluate when a new front-end scan compares operator-controlled strings,
when bundle member limits change, when exception-selector indexing changes, or
when the regex engine or its NFA unrolling of counted repetition changes.

## Resolution references

- F-S1-1, F-S1-2, F-S1-3 (second-pass S1 rule-set review, 2026-11-07), fixed by
  `fix(waf): charge bundle dedupe, forward selector scans and nested quantifiers before work`.
