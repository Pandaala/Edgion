# B0028 — LB Selection Facade

## Scope

- Baseline: `be90cfc13133c6b8cd9fd00dca298936e38b5280`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- File: `edgion-gateway/src/lb/selection/mod.rs`
- Allowed code files: none; analysis concluded examined-no-change.
- Forbidden files: every source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: B0027/B0028 campaign ledger, batch, main, and changelog records only.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0028 analysis turn.
- Decision: `examined-no-change`. The facade exposes exactly ConsistentHash, P2C, and RoundRobin; their sole production algorithm consumer is the canonical compiled-selector layer.
- Ownership evidence: RR and CH own cold immutable selectors, while P2C is the single allocation-free chooser shared by LeastConn and EWMA. The EndpointSlice Pingora RoundRobin type is a compatibility marker, not a second selection implementation.
- B0027 revalidation: CH and RR intentionally keep local equal numeric limits because Ketama's node cast and RR's schedule representation constrain different candidate populations and may evolve independently.
- Rejected churn: narrowing module visibility, adding root re-exports or prose, or hoisting a shared count constant would not improve correctness, reliability, performance, security, or regression coverage.
- Freshness revalidation: valid at `be90cfc13`, including the committed B0027 cap and tests.

## Implementation

- Implementation: none. No material improvement justified a source or documentation edit.

## Independent review

- Reviewer: `b0001_review`, independent from the B0028 analyst.
- Verdict: approved examined-no-change with no findings after checking exports, all consumers, ownership, compatibility markers, dependency direction, public visibility, test coverage, and B0027 limit separation.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | Complete selection 33 and compiled-selector 8 tests passed. |
| Independent review checks | passed | Gateway check, formatting, no-tracing, consumer/cycle searches, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
