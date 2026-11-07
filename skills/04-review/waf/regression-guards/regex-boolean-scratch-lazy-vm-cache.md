---
name: regex-boolean-scratch-lazy-vm-cache
description: Use when reviewing RegexBooleanScratch cache construction, the owner/exact/guard/VM order in CompiledRegexProgram::is_match and CompiledMatcher::scan_inner, or boolean scratch admission.
knowledge_type: regression-guards
---

# Boolean regex scratch builds its VM cache lazily

## Conclusion and scope

`RegexBooleanScratch` in `edgion-waf/src/matcher.rs` holds
`cache: Option<PikeCache>`. `CompiledRegexProgram::create_scratch` sets it to
`None`, and the private `RegexBooleanScratch::cache_mut` builds it with
`get_or_insert_with` immediately before the first VM call. Later calls within the
same transaction reuse the retained cache. Reached programs that never run the
VM (exact hints, guard misses) no longer pay NFA-sized cache construction.

Both entry points keep one order: owner token (`Arc::ptr_eq`), then exact hint,
then literal guard, then VM.

- `CompiledRegexProgram::is_match` returns `ScratchShape` for foreign scratch
  before any shortcut or cache access, then answers from `exact_match` for
  `Simple` hints, skips the VM when `may_match` rejects a 64-byte-or-longer
  guard miss, and only then calls `vm.is_match(scratch.cache_mut(..), ..)`.
- `CompiledMatcher::scan_inner` does the same around
  `which_overlapping_matches`. Reference (non-accelerated) scans always reach the
  VM, so a scratch first used only by accelerated scans initializes its cache on
  the first reference scan, exactly once.

## Rationale and evidence

Per-transaction scratch slots were already created lazily
(`lazy_boolean_scratch` / `lazy_group_scratch` in `seclang/transaction/selection.rs`); the
remaining eager cost was the `vm.create_cache()` inside each slot, which
regex-automata sizes to the NFA.

Preserved invariants:

- Admission is unchanged and conservative. `RegexCore::compile_many` still
  builds a probe cache and charges `size_of::<RegexBooleanScratch>()` plus the
  full cache envelope per program, and the transaction's
  `matcher_scratch_capacity` still charges every boolean slot. Do not switch to
  charging only caches that were built. Both sites read `size_of` at runtime, so
  they follow any wrapper size change automatically; at the time of the change
  the wrapper measured 224 bytes before and after (`Option` uses the cache's
  niche). No test pins that number.
- `RegexCaptureScratch` stays eager; capture programs always run the VM.
- Scratch remains per transaction and per program generation; no cache is shared
  across requests or generations. Match results, fanout, source order and error
  ordering are unchanged.

## Alternatives and limits

Rejected: a `#[cfg(test)]` accessor in `src/` (the unit-test layout guard
forbids it; the `#[path]`-mounted test module reads the private field).
Rejected: a `measurements::Event` counter (would cross `matcher` into
`seclang`; `Option::get_or_insert_with` already guarantees once-only
construction). Capture scratch laziness is out of scope.

## Verification and re-evaluation

Local unit tests in `edgion-waf/tests/unit/matcher_test.rs`: exact-hint and
guard-miss inputs leave `cache` as `None`; a VM-requiring input builds it; lazy
results equal a fresh eager `PikeVM` cache over
hinted, guarded and plain fixtures; foreign scratch errors and leaves `cache`
unbuilt; an accelerated exact scan followed by a reference scan builds the cache.
Once-only construction is guaranteed by `Option::get_or_insert_with`, not by a
test: the cache lives inline in the scratch, so an address comparison cannot
observe a rebuild.
The first-use epsilon-growth test reads the really built cache and still asserts
it fits the admitted bound. Restoring the eager constructor makes five tests
fail. These are not throughput or RSS measurements.

Re-evaluate when a new VM entry point is added for boolean programs, when the
owner/exact/guard/VM order changes, when compile-time or transaction scratch
accounting changes, or if capture scratch is made lazy.

## Resolution references

- Former `tasks/todo/waf-15-regex-vm-cache-initialization.md`, completed
  2026-11-07.
