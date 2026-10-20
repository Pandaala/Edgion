# WAF multiMatch stage ownership

Status: deferred representation and evaluation-order decision for the user.
Source baseline: `624b9df97`. No runtime or dependency change.

## Evidence

Ordinary Gateway WAF rule evaluation reaches
transaction.rs::evaluate_single_input_with_transform_memo. Its multiMatch branch
calls transform.rs::apply_transformations_multimatch before evaluating any
predicate. MaterializedStages fallibly reserves the outer vector, then copies
the raw input and every changed transformation stage into separate owned vectors.
Unchanged transformation stages are not added. The walker completes validation
of the entire recipe before returning the stage list.

The consuming loop evaluates stages in order and returns on the first match.
Consumed stage buffers drop as iteration advances; remaining stages stay owned
until consumed or the iterator drops. This is temporary per-input storage, not
retention for the whole transaction. During collection it coexists with transform
scratch. No measured peak-heap or throughput result is available for this audit.

The ordering is required by the current contract: the transaction test
single_input_validates_later_transform_before_an_early_multimatch_hit expects a
later hex-encode output-limit failure even when raw input would already match.
No matched variable is committed in that case. Predicates also charge work and
can mutate matcher scratch, scan caches and capture/budget state. Replacing the
stage list with immediate early-exit callbacks is not behavior-preserving.

## Decision

Keep the current stage list, borrow only the immutable initial stage while owning
changed stages, or design provisional predicate evaluation/replay that avoids
retaining full intermediate payloads. The first borrowing option is narrower,
but still changes representation/metadata and an allocation-failure opportunity;
it does not eliminate changed-stage storage. A provisional design has additional
failure-order and accounting obligations and requires separate implementation
planning. The fused JSON collector provides related mechanisms, not a drop-in
replacement for ordinary evaluation.

This is separate from waf-06's marker-free decoder identity decision. Avoid
combining them implicitly: one changes stage ownership/evaluation, the other
changes which transformation steps materialize scratch and qualify for a memo.

## Acceptance for an approved implementation

Preserve raw-first and changed-stage ordering, duplicate suppression, first-match
selection, negated operators, captures and matched-value/name ownership. Validate
all transformations before publishing a semantic predicate outcome. A later
transformation error must still beat an earlier match or predicate error; do not
bypass output/work limits after a provisional result.

Review allocation/arithmetic failures separately from semantic failures. Specify
when provisional physical work, scratch/cache mutation and capture admission may
occur, how pending outcomes are discarded, and which existing budgets observe
them. Do not hide physical work with logical-counter rollback or treat a borrowed
stage as permission to retain a scratch slice after the walker reuses its buffer.

Compare empty recipes, unchanged/changed prefixes, early/late/no matches, negation,
late transformation failures, capture and operator limits, and exact boundaries.
Retain the existing ordering regressions and compare ordinary and JSON paths
without unifying their ownership by assumption. Measure raw/changed-stage payload,
metadata, scratch high-water and execution cost across realistic recipe lengths.

Keep the canonical transform walker and operator implementation. No second body
cache, full-response storage, request tracing, payload logs or new metrics are
needed. The Gateway remains the anchor; future shared-leaf changes need an exact
supporting-file allowlist and IMPL plan/final reviews. Keep integration fixtures
unchanged. This task grants no implementation authority before the user's decision.

## Knowledge and verification limits

See [fused JSON inspection](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
for the current ordinary intermediate-stage and provisional-observer boundaries.
Current tests verify current behavior, not a proposed streaming implementation.
No runtime change or benchmark was performed by this documentation audit.
