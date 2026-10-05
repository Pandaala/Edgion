# B0005 — Degradation Threshold

## Scope

- Baseline: `4237d2c21a52fd408b75fbf4937de27d1253b86b`
- Pipeline state: `complete`
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
- Freshness revalidation: valid at `4237d2c21`; B0004 changed neither code file nor the relevant lock/publication ordering.

## Implementation

- Implementation subagent: `b0002_analysis` in separate plan and implementation turns.
- Complexity: complex; implementation plan and independent plan review required after B0004 commits.
- Approved plan: production refresh accepts only a service key, acquires the
  coordinator state mutex, then lazily resolves the canonical threshold and
  republishes through the existing membership-fenced snapshot path. The
  all-services refresh acquires the mutex once and calls only a private
  lock-held helper. A direct-value publisher remains test-only for existing
  selector coverage.
- Regression plan: exercise the same private lazy-resolver seam with a local
  coordinator; the resolver asserts `state.try_lock().is_none()`, returns a
  sentinel threshold, and the test verifies that value reaches the real runtime
  snapshot. This deterministically proves lock-before-resolve without sleeps,
  polling, thread scheduling, or global refresh side effects.
- Actual implementation: completed exactly as approved. Override updates no
  longer precompute a threshold; single-service and all-services refreshes
  resolve lazily under coordinator serialization. The direct-value publisher
  is compiled only for tests, and the deterministic lock-order regression was
  added to the coordinator unit suite.

## Plan review

- Reviewer: `b0001_adjudicate`, independent from the B0005 analyst and implementer.
- Initial verdict: changes requested because the original threaded test could
  start after the source mutation and allow the old ordering to pass.
- Final verdict: approved after replacing timing assumptions with the lazy
  resolver/`try_lock` proof and explicitly forbidding recursive acquisition in
  the all-services path.

## Independent review

- Reviewer: `b0001_review`, independent from the B0005 analyst, implementer,
  and plan reviewer.
- Verdict: approved with no findings. The reviewer confirmed the race is
  removed, lock order cannot cycle, generation/membership fencing and snapshot
  state are preserved, the test is deterministic, and the exact two-file code
  scope is respected.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths | passed | Baseline unchanged before delegation. |
| Focused implementation checks | passed | Degrade, coordinator, selector, backend-policy, and Gateway-config unit suites; Gateway all-target check; formatting, no-tracing, English-only, and diff checks passed. |
| Independent review checks | passed | 23 degrade-related tests, dedicated lock-order regression, Gateway lib check, formatting, agent-doc, no-tracing, English-only, and diff checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `b47f6b0acd0cf99c2e973648821c60f0580a923f`.
