# B0021 — LB EWMA Facade

## Scope

- Baseline: `0d78a3294397f2fa3a731e4b3ab20cfb6744cba2`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/lb/ewma/mod.rs`
- Allowed code files: none; analysis concluded examined-no-change.
- Forbidden files: every source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: B0020/B0021 campaign ledger/batch/main/changelog records only.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0021 analysis turn.
- Decision: `examined-no-change`. The facade truthfully locates score state on `LbBackend`, selection in P2C, and the canonical estimator in `score`; the removed global alpha surface remains absent.
- Decisive evidence: one response classifier samples at response headers, eager compiled selectors retain immutable ready backend Arcs, P2C handles empty/single/multi and bounded healthy fallback, fresh selection time and captured header time serve their distinct roles, and attempt guards balance the connection multiplier.
- Rejected churn: narrowing module visibility or adding re-exports has no material correctness, performance, or API value in this unpublished crate.
- Preserved semantics: B0020 monotonic write anchors, shared score/load state, generation identity, health filtering, sampling, and lock-free bounded request selection.
- Freshness revalidation: valid at `0d78a3294`; B0020 changed score internals, algorithm canon, and campaign records while preserving this facade and call-chain ownership.

## Implementation

- Implementation: none. No material correctness, reliability, performance, security, or regression-coverage improvement justified a source edit.

## Independent review

- Reviewer: `b0001_review`, independent from the B0021 analyst.
- Verdict: approved examined-no-change with no findings after checking the full estimator, sampling, selection, backend, attempt, and publication chain.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Analysis checks | passed | EWMA 21, P2C 5, and compiled-selector 8 tests passed. |
| Independent review checks | passed | EWMA, sample, P2C, compiled-selector, response-filter, logging, backend, attempt, selector, and coordinator tests plus Gateway library check, formatting, and no-tracing passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; the first cargo-deny session was retried after a no-output tool hang, and the retry passed with only pre-existing warnings. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
