# Regex detection and capture search reuse

Status: deferred execution and budget decision for the user.
Source baseline: `011c6ffd9`. No runtime or dependency change.

## Evidence

Gateway WAF predicate execution reaches
`inspect_predicate_stage_with_hit_matcher` in `seclang/transaction.rs`.
Without an optional hit matcher, `evaluate_operator` dispatches a Capture regex
to `CompiledCaptureRegexProgram::is_match`. Successful non-negated predicates
then pass `before_captures` and enter `extract_captures_into`, which performs
capture allocation/capacity admission before calling the program's `captures`.
Captured ranges supply numeric keys and optional named aliases.

These are two search invocations, not necessarily two full or identical scans.
The pinned regex-automata 0.4.18 PikeVM `is_match` uses `earliest(true)` and an
empty output-slot slice. `captures` searches with capture slots and resets the
recorded pattern first. Boolean detection therefore has no reusable group spans;
reading existing scratch captures afterward could expose an earlier result.

Eligible structured-body capture-hit batches replace individual boolean searches
with a shared matcher; original programs still own extraction. Reference mode
retains individual detection. Negation does not extract captures, and
`evaluate_prefix_member` only needs detection. `group_by_name` is not used by
these paths: extraction iterates numeric indexes and borrows names.

## Existing decision and re-evaluation

The original fused JSON guard already records a rejected single-search prototype:
all-miss timing regressed in both run orders, so positive capture execution
deliberately retained boolean miss checking followed by extraction on hits.
This is historical evidence, not a benchmark rerun in this audit. Retaining the
current path is the established baseline; reconsider it only with new workload
evidence or a materially different bounded design that addresses that regression.

Retain early boolean detection plus conditional extraction, or design bounded
result reuse to avoid repeated search work on direct positive capture predicates.
No replacement is selected. Measure miss-heavy and hit-heavy traffic, early/late
matches, greedy/alternative patterns, capture counts, input sizes, first/reused
scratch and optional hit-batch participation. Compare complete predicate execution,
not isolated calls. No throughput, allocation or memory benefit is established.

A fused capture search is not an authorized drop-in optimization. The existing
[capture-hit contract](../../skills/04-review/waf/regression-guards/fused-json-inspection.md#optional-capture-hit-batches)
forbids capture extraction on misses. Searching capture slots before knowing a
match exists needs an explicit reviewed revision of that boundary. It can lose
early termination and add work on misses or detection-only paths.

The collector's `before_captures` charges input bytes. Capture allocation and
capacity admission can fail before the current capture search. Moving search
ahead of these gates changes physical execution order even with identical
verdicts. Preserve logical charges, checkpoint identity, partial usage and sticky
failure behavior, or explicitly approve a changed contract. Unchanged return
values alone do not establish budget-neutral execution.

## Acceptance for an approved implementation

- Preserve LeftmostFirst spans, optional unmatched groups, empty matches,
  numeric/named alias order, byte semantics, negation, TX/actions and multiMatch.
  Retain prefix-only detection and optional shadow eligibility.
- Authenticate reused results against exact program, input and stage. Preserve
  foreign-scratch rejection and clear stale evidence on misses, errors, scratch
  reuse and mode changes. Add no payload copy to retain proof.
- Preserve conservative scratch envelopes and aggregate retained admission;
  reconcile wrapper/slot accounting if layouts change. Exercise exact and
  one-below work, capture and physical budgets, including refusal between
  detection, allocation admission and extraction.
- Compare ordinary predicates and JSON/XML replay in Accelerated and Reference
  modes, with optional hit plans available, absent and ineligible. Cover repeated
  fields, profile changes and whole/chunked schedules. Retain existing fixtures.

This differs from [WAF-09](waf-09-profile-capture-hit-index.md) profile indexing
and [WAF-15](waf-15-regex-vm-cache-initialization.md) boolean cache initialization.
Neither is authorized here. Freeze the supporting-file allowlist and obtain
independent IMPL plan/final review before implementation. Keep canonical execution
ownership; add no body cache, response aggregation, request tracing, payload logs
or profiling metrics.

## Sources and verification limits

Source owners under `edgion-waf/src/` are `matcher.rs` capture wrappers,
`seclang/transaction.rs` predicate/extraction paths and
`seclang/json_inspection_collector.rs` observer gates. Existing matcher tests cover
capture ownership/bounds; transaction and collector tests cover observer admission
and optional hit-batch searches. These validate current code, not fused search.

Fresh unchanged-production WAF tests passed 869 cases with six existing ignored.
Formatting and workspace all-target check/Clippy passed with existing warnings.
No prototype, benchmark, full-workspace suite or live Gateway integration ran.
This records a decision for later review, not a defect or implementation mandate.
