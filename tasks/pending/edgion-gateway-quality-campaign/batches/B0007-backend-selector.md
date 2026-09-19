# B0007 — Backend Selector

## Scope

- Baseline: `b47f6b0acd0cf99c2e973648821c60f0580a923f`
- Pipeline state: `analysis-ready`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/backends/selector.rs`
- Allowed code files: `edgion-gateway/src/backends/selector.rs` only.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the code file above and campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0007 analysis turn.
- Confirmed defect: sticky-session address restoration searches canonical all-slot affinity state but computes eligibility only from live health/outlier state. An EndpointSlice endpoint that remains published with `ready=false` can therefore bypass the ordinary ready-only selector.
- Decision: `change` (simple). Keep all-slot token recognition, but require `slot.ready` in the canonical restoration eligibility gate so strict mode can recognize the pinned endpoint as unhealthy and fail closed.
- Preserved semantics: canonical address matching, section scoping, normal ready-only selection, generation/runtime fencing, strict/non-strict persistence behavior, health/outlier eligibility, schema, logs, metrics, and protocol behavior.

## Implementation

- Implementation subagent: pending.
- Complexity: simple; no plan-review stage required after freshness validation.

## Independent review

- Reviewer: pending.
- Verdict: pending.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Baseline selector suite | passed | 10 selector tests passed during analysis. |
| Frozen integration paths | passed | Existing integration files remained unchanged during analysis. |

## Handoff

- Ledger/changelog updated: no.
- Commit: pending automatic commit.
