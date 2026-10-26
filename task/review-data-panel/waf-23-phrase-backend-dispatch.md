# Native phrase backend dispatch

Status: conditional investigation and design decision for the user.
Source baseline: `b0eb720e3`. No runtime change or prototype.

## Evidence and scope

Active WAF Matcher operands reach native phrase pattern-set scans through
`seclang/transaction.rs` and `matcher.rs`; CapturePhrase operands reach native
leftmost search. The immutable AhoCorasick owner stores one Backend selected at
construction. `aho_corasick/mod.rs` search loops call Backend::next_state per
consumed byte and traverse outputs through a borrowed OutputIter. That iterator
calls Backend::next_output; span construction also calls Backend::pattern_length.
These methods in `aho_corasick/nfa/mod.rs` match Sparse, Contiguous or Dfa.

The source therefore expresses repeated backend dispatch for a backend that
cannot change during a search. This is not proof of repeated machine branches:
inlining, specialization or loop optimization may already remove them. Nor does
source inspection establish a bottleneck. No owned payload or output-list copy
was found at these dispatch boundaries; the candidate is execution structure.

## Decision gate

Keep the current shared loops unless release code inspection and representative
profiling show material remaining dispatch cost. Only then compare dispatch once
at the search entry into a common specialized kernel. Measure instruction count,
latency, code size and instruction-cache effects, including short inputs and
prefilter-dominated searches. Extra monomorphized code and compile complexity can
outweigh saved branches. No alternative is selected or authorized here.

Keep one canonical search algorithm rather than maintaining separate handwritten
loops for each backend. Any internal interface must cover state transitions,
ordered outputs and pattern lengths coherently; specializing transitions alone
can leave other dispatch in the inner loop. Avoid dynamic trait calls that merely
replace enum dispatch, new shared scratch, unsafe code or an extra matcher cache.

Include Sparse, Contiguous, Dfa and Auto selections, both case modes, sparse/dense
outputs, misses/hits, common-prefix and suffix-heavy dictionaries, different
input lengths and prefilter on/off. Compare end-to-end SecLang work as well as
isolated scanning. Do not extrapolate an explicit-DFA microbenchmark to Gateway
traffic or claim memory savings from changing control flow.

## Acceptance for an approved implementation

Preserve backend selection and its exact limit/error behavior, compilation and
scratch accounting, source-order duplicates, empty root outputs, failure/output
links, first/longest tie rules and early termination. Preserve byte folding,
input ranges, anchors, earliest searches, reference execution and all candidate
confirmation/adaptive diagnostics. Validate input and PatternSet shape before
clearing results, as today; retain error identity and partial-state ordering.

Keep logical work charges and aggregate admission even if execution becomes
faster. Verify current all-backend oracle/parity tests plus directed invalid-input,
reuse, empty/overlapping/duplicate-pattern and exact/one-below budget cases.
Changes affecting shared iterators require their own public-API and stream tests;
the direct contiguous-input inspection is not proof of those contracts.

This is distinct from the conditional
[DFA state-layout task](../../tasks/todo/waf-dfa-state-layout.md), which concerns
row-index arithmetic, and [SIMD search](../../tasks/todo/waf-native-simd-search.md).
It does not activate either task. The dormant Gateway raw stream lane remains
outside the performance premise; see [WAF-16](../../skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md#dormant-streaming-projection-decision).
Freeze the exact supporting-file allowlist and obtain independent IMPL plan/final
review before implementation. Add no body cache, response aggregation, request
tracing, payload logs or profiling metrics.

## Verification limits

Sources are `edgion-waf/src/aho_corasick/mod.rs`, `nfa/mod.rs`,
`nfa/sparse.rs`, `nfa/contiguous.rs` and `dfa.rs`, together with the transaction
and matcher callers above. Existing all-backend comparisons live in
`edgion-waf/tests/unit/aho_corasick/mod_test.rs`.

Fresh unchanged-production WAF library tests passed 871 cases with six existing
ignored. Formatting and workspace all-target check/Clippy passed with existing
warnings. No code-generation experiment, benchmark, full-workspace suite or live
Gateway integration ran. The current dispatch remains valid; reconsideration is
conditional on new evidence, not an unfinished implementation requirement.
