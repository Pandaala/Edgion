---
name: regex-boolean-scratch-lazy-vm-cache
description: Build the boolean-regex PikeVM cache only before the first VM call while keeping owner checks first and admission charging the full envelope.
knowledge_type: regression-guards
---

# Boolean regex scratch builds its VM cache lazily

waf-15 removes NFA-sized cache construction from reached programs that never run
the VM. `RegexBooleanScratch` in `edgion-waf/src/matcher.rs` holds
`cache: Option<PikeCache>`; `CompiledRegexProgram::create_scratch` sets it to
`None`, and the private `RegexBooleanScratch::cache_mut` builds it with
`get_or_insert_with` immediately before the first VM call. Later calls within the
same transaction reuse the retained cache.

Both entry points keep one order: owner token (`Arc::ptr_eq`) → exact hint →
literal guard → VM.

- `CompiledRegexProgram::is_match` returns `ScratchShape` for foreign scratch
  before any shortcut or cache access, then answers from `exact_match` for
  `Simple` hints, skips the VM when `may_match` rejects a 64-byte-or-longer
  guard miss, and only then calls `vm.is_match(scratch.cache_mut(..), ..)`.
- `CompiledMatcher::scan_inner` does the same around
  `which_overlapping_matches`. Reference (non-accelerated) scans always reach the
  VM, so a scratch first used only by accelerated scans initializes its cache on
  the first reference scan, exactly once.

Preserved invariants:

- Admission is unchanged and conservative. `RegexCore::compile_many` still
  builds a probe cache and charges `size_of::<RegexBooleanScratch>()` plus the
  full cache envelope per program, and the transaction's
  `matcher_scratch_capacity` still charges every boolean slot. Do not switch to
  charging only caches that were built.
  `Option<PikeCache>` uses the cache's niche, so the wrapper stays 224 bytes.
- `RegexCaptureScratch` stays eager; capture programs always run the VM.
- Scratch remains per transaction and per program generation; no cache is shared
  across requests or generations. Match results, fanout, source order and error
  ordering are unchanged.

Evidence is local unit tests in `edgion-waf/tests/unit/matcher_test.rs`, which
read the private field directly (no test accessor in `src/`): exact-hint and
guard-miss inputs leave `cache` as `None`; a VM-requiring input builds it once
and keeps its address; lazy results equal a fresh eager `PikeVM` cache over hinted,
guarded and plain fixtures; foreign scratch errors and leaves `cache` unbuilt; an
accelerated exact scan followed by reference scans builds one cache. The
first-use epsilon-growth test reads the really built cache and still asserts it
fits the admitted bound. Restoring the eager constructor makes the laziness tests
fail. These are not throughput or RSS measurements.

Re-evaluate when a new VM entry point is added for boolean programs, when the
owner/exact/guard/VM order changes, when compile-time or transaction scratch
accounting changes, or if capture scratch is made lazy.
