---
name: regex-boolean-scratch-lazy-vm-cache
description: Use when reviewing RegexBooleanScratch or RegexCaptureScratch cache construction, the owner/exact/guard/VM order in CompiledRegexProgram::is_match, CompiledCaptureRegexProgram::is_match and CompiledMatcher::scan_inner, the capture decision VM, or regex scratch admission.
knowledge_type: regression-guards
---

# Regex scratch builds its VM cache lazily; capture programs decide without capture tracking

## Conclusion and scope

All single-program regex paths in `edgion-waf/src/matcher.rs` follow one order:
owner token (`Arc::ptr_eq`), then exact hint, then literal guard, then VM. The
VM cache is built immediately before the first VM call and reused for the rest
of the transaction. Reached programs that never run the VM (exact hints, guard
misses) pay no NFA-sized cache construction.

- `CompiledRegexProgram::is_match` returns `ScratchShape` for foreign scratch
  before any shortcut or cache access, answers from `exact_match` for `Simple`
  hints, skips the VM when `may_match` rejects a 64-byte-or-longer guard miss,
  and only then calls `vm.is_match(scratch.cache_mut(..), ..)`.
  `RegexBooleanScratch` holds `cache: Option<PikeCache>`.
- `CompiledMatcher::scan_inner` does the same around
  `which_overlapping_matches`. Reference (non-accelerated) scans always reach
  the VM, so a scratch first used only by accelerated scans initializes its
  cache on the first reference scan, exactly once.
- `CompiledCaptureRegexProgram` owns a `decision: CompiledRegexProgram` built
  from the same HIR with `WhichCaptures::None` and sharing the capture
  program's owner token. `is_match` checks the owner, then the decision
  program's exact hint and literal guard, then runs the capture-free decision
  VM. The capture VM (`WhichCaptures::All`) runs only in `captures`, which the
  operator path calls only after a positive, non-negated match of a member
  with a `capture` action (`extract_captures_into`) or after a capture-hit
  matcher hit.
- `RegexCaptureScratch` holds `cache: Option<PikeCache>` and
  `captures: Option<Captures>`, both built lazily. The single cache is always
  created from the capture VM (the admitted envelope) and reset to the decision
  VM, whose NFA never has more states (checked at compile time,
  `ProjectionInvariant` otherwise). The first `captures` call resets it back
  inside the same capacity and builds the slots; from then on
  `captures.is_some()` means the cache is capture-sized and the capture VM also
  answers later decisions for that program. No transition reallocates, so a
  program costs at most the old eager allocations and decision-only programs
  save the capture slots.

## Rationale and evidence

Profile H1 in `docs/superpowers/plans/2026-11-07-waf-second-pass-baseline.md`:
`CompiledCaptureRegexProgram::is_match` ran the `WhichCaptures::All` PikeVM with
no shortcuts and was 28 % (clean_get) to 82 % (big_cookie_8k) of request CPU.
`captures()` after a hit was under 1 %. Capture states sit in every unanchored
start closure, so removing them from the decision NFA is the measured win
(task T12a, commit body carries the CSV rows): `transaction_crs` p50 improved
on every case (big_cookie_8k -7.9 %, post_json_4k -4.4 %, post_form_2k -3.7 %,
lfi_path -0.4 % within noise), allocations dropped by 81-95 per request, and the
result fingerprints and the FTW inventory stayed identical. The win is modest:
the remaining capture-program cost is the PikeVM itself. A variant that
kept only the shortcuts on the capture NFA (no second NFA) showed no p50 gain,
because CRS capture regexes almost never qualify for an exact hint or literal
guard (2 simple and 5 guarded out of 189 in the supported bundles).

Preserved invariants:

- Admission is unchanged and conservative. `RegexCore::compile_many` still
  builds a probe cache and charges the scratch wrapper plus the full cache
  envelope per program; capture programs also charge their capture slots and
  the epsilon stack of the larger capture NFA. `matcher_scratch_capacity`
  still charges every slot. Do not switch to charging only caches that were
  built. The lazy `RegexCaptureScratch` measured 264 bytes, equal to the former
  eager layout (`Option` uses each member's niche);
  `capture_scratch_admission_is_not_larger_than_the_eager_layout` pins
  "not larger".
- The decision NFA is real retained memory. It is charged at compile time:
  `memory_usage`/`engine_memory_usage` include it, `retain_matcher` charges it
  in `compile_capture_regex`, `conservative_program_memory_bound` adds a full
  `max_nfa_bytes` bound for it, and the build peak adds one `max_nfa_bytes`
  term. Its optional hint is installed last in `publish.rs` through
  `decision_hint_target`, inside the program cap and the generation budgets.
  On the supported CRS bundles this added about 3.4 MB (+37 %) to
  `CompiledGeneration::retained_bytes()`.
- Scratch remains per transaction and per program generation; no cache is
  shared across requests or generations. Match results, capture spans, fanout,
  source order and error ordering are unchanged.

## Alternatives and limits

Rejected: a `#[cfg(test)]` accessor in `src/` (the unit-test layout guard
forbids it; the `#[path]`-mounted test modules read private fields or use
test-only `impl` blocks such as `RegexCaptureScratch::test_built`). Rejected:
a `measurements::Event` counter (would cross `matcher` into `seclang`;
`Option` construction already guarantees once-only builds). Rejected: a
second, separately built capture cache (precharge would grow) and a mode flag
that resets back to the decision VM after a capture search (the wrapper would
grow by a word per program). Not attempted here: a literal prefilter for the
PikeVM; the remaining capture-program cost is the PikeVM itself.

## Verification and re-evaluation

Local unit tests in `edgion-waf/tests/unit/matcher_test.rs`. Boolean: exact-hint
and guard-miss inputs leave `cache` as `None`; a VM-requiring input builds it;
lazy results equal a fresh eager `PikeVM` cache over hinted, guarded and plain
fixtures; foreign scratch errors and leaves `cache` unbuilt; an accelerated
exact scan followed by a reference scan builds the cache. Capture:
`capture_guard_miss_and_exact_hint_skip_the_vm`,
`capture_decisions_without_capture_search_never_build_capture_slots`,
`capture_groups_and_decisions_equal_eager_reference_across_mode_switches`
(including ten groups, TX.0..TX.9), `capture_decision_vm_is_capture_free_and_never_larger`
and `capture_scratch_admission_is_not_larger_than_the_eager_layout`.
`edgion-waf/tests/unit/seclang/transaction/mod_test.rs`
`capture_programs_build_capture_slots_only_when_captures_are_staged` checks the
operator path: a matching capture member builds the slots, a negated one only
decides, and a member without `capture` compiles to a boolean program. These
are not throughput or RSS measurements.

Re-evaluate when a new VM entry point is added for regex programs, when the
owner/exact/guard/VM order changes, when compile-time or transaction scratch
accounting changes, when the capture extraction call sites change, or when
regex-automata changes `Cache::reset` capacity behavior.

## Resolution references

- Former `tasks/todo/waf-15-regex-vm-cache-initialization.md`, completed
  2026-11-07.
- Task T12a in `docs/superpowers/plans/2026-11-07-waf-second-pass.md`
  (hotspot H1), completed 2026-11-07.
