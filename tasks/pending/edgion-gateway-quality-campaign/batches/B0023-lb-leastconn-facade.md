# B0023 — LeastConn Facade

## Scope

- Baseline: `5b40da9b2114cb676492f9c6d2a2cecd48a27f74`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/lb/leastconn/mod.rs`
- Allowed code files: none; analysis concluded examined-no-change.
- Forbidden files: every source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: B0022/B0023 campaign ledger/batch/main/changelog records only.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0023 analysis turn.
- Decision: `examined-no-change`. The facade privately owns the cleaner module, re-exports the sole `BackendCleaner`, locates connection state on `LbBackend`, and points selection to canonical P2C.
- Decisive evidence: one production constructor/start site transfers the cleaner to one shutdown watcher that awaits `stop`; duplicate start and restart are covered. LeastConn scores shared `conn()` while retired-generation cleanup polls `owned_conn()`, with B0019 preventing stale-wrapper count theft.
- Rejected churn: visibility/re-export rearrangement and removal of cheap construction/Drop tests offer no material quality gain.
- Preserved semantics: empty/single/multi P2C, health fallback, attempt accounting, counter sharing, exact-generation draining, cancellation, token reset, and restart.
- Freshness revalidation: valid at `5b40da9b`; B0022 changed cleaner INFO fields, runtime-state comments, review knowledge, and campaign records without altering facade ownership.

## Implementation

- Implementation: none. No material correctness, reliability, performance, security, or regression-coverage improvement justified a source edit.

## Independent review

- Reviewer: `b0001_review`, independent from the B0023 analyst.
- Verdict: approved examined-no-change with no findings after checking facade/API, lifecycle, selector, accounting, draining, and test value.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Analysis checks | passed | LeastConn 6, P2C 5, and attempt 7 tests passed. |
| Independent review checks | passed | LeastConn, P2C, attempt, backend, runtime-state, compiled-selector, shutdown, logging, and coordinator tests plus Gateway library check, formatting, no-tracing, and diff checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
