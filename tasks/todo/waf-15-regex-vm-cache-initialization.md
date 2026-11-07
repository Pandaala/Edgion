# Construct the WAF boolean-regex PikeVM cache only when the VM actually runs

Priority: P3   Area: plugin:waf (edgion-waf matcher)

## Problem
Each WAF transaction lazily creates a per-program scratch slot on first use
(`lazy_boolean_scratch` / `lazy_group_scratch` in
`edgion-waf/src/seclang/transaction.rs`). But inside that slot,
`CompiledRegexProgram::create_scratch` (`edgion-waf/src/matcher.rs`) and
`CompiledMatcher::create_scratch` (its `regex_scratch` field) always build a
`PikeCache` via `vm.create_cache()`. regex-automata's `Cache::new` sizes sparse
sets and slot tables to the NFA, i.e. several allocations proportional to the
program size.

Many reached programs never call the VM:
- `CompiledRegexProgram::is_match` returns from `exact_match` for `Simple` hints
  (plain literal with anchors) without touching `scratch.cache`.
- `may_match` skips the VM for inputs >= 64 bytes when the literal guard misses.
- `CompiledMatcher::scan_inner` (accelerated) takes the same exact/guard paths.

So every request on a CRS-like ruleset pays NFA-sized cache construction for
every reached literal-hinted program, per transaction.

## Proposed change
Make the VM cache inside `RegexBooleanScratch` lazy (`Option<PikeCache>`, or a
small helper `cache_mut(&mut self, vm) -> &mut PikeCache` using
`get_or_insert_with`). Construct it immediately before the first
`vm.is_match` / `vm.which_overlapping_matches` call and retain it for later
scans within the same transaction. Apply to both `CompiledRegexProgram::is_match`
and `CompiledMatcher::scan_inner`. Capture-regex scratch is out of scope.

## Must preserve
- Scratch owner validation (`Arc::ptr_eq` token check) runs before any
  exact/guard shortcut; foreign scratch still errors.
- Reference (non-accelerated) grouped scans still run the VM, including when
  reusing scratch previously used only by accelerated scans.
- Admission/accounting stays conservative and unchanged: `RegexCore::compile`
  and transaction scratch accounting keep charging the full cache envelope per
  program (do not switch to charging only reached/used caches). Reconcile the
  wrapper size if `Option` changes `size_of::<RegexBooleanScratch>()`.
- Match results, fanout, source order and error ordering are unchanged; no
  scratch shared across requests or generations.

## Verification
- `cargo test -p edgion-waf matcher` and the transaction lazy-slot admission tests.
- Add unit tests: exact-hint and guard-miss inputs leave the cache uninitialized;
  a subsequent VM-requiring input (short input / Reference scan) initializes it
  once and matches identically to the eager version.
