# B0005 — Degradation Threshold

## Scope

- Baseline: `9f48daa0cc0e4308456daf5ca37dfefe71e99f8d`
- Pipeline state: `analysis-ready`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/backends/degrade_threshold.rs`
- Allowed code files: `edgion-gateway/src/backends/degrade_threshold.rs` and `edgion-gateway/src/backends/discovery/coordinator.rs`.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the two code files above and campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0005 analysis turn.
- Confirmed defect: effective thresholds are resolved before the coordinator state lock, so concurrent global/override updates can publish a stale value into the live runtime snapshot indefinitely.
- Decision: `change` (complex, plan-first). Acquire coordinator state before resolving the canonical effective threshold; production callers must not pass precomputed values.
- Preserved semantics: threshold ranges/defaults, zero-disable behavior, validation, captured HTTP/gRPC retry overrides, live health/outlier/counter ownership, schema, logs, metrics, and protocol behavior.

## Implementation

- Implementation subagent: pending.
- Complexity: complex; implementation plan and independent plan review required after B0004 commits.

## Independent review

- Reviewer: pending.
- Verdict: pending.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths | passed | Baseline unchanged before delegation. |

## Handoff

- Ledger/changelog updated: no.
- Commit: pending automatic commit.
