# B0022 — LeastConn Backend Cleaner

## Scope

- Baseline: `4ad1de05c0a548e16c61c47c7289b836e2fc3975`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/lb/leastconn/cleaner.rs`
- Allowed files: the anchor, comment-only `edgion-gateway/src/lb/runtime_state/mod.rs`, and `skills/04-review/concurrency/review-evidence/lb-false-positives.md`.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the three implementation/knowledge files above and B0021/B0022 campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0022 analysis turn.
- Confirmed defect: the drain-completion INFO event emitted a full backend IP:port, violating the rule that backend topology remains debug-only. B0004 had already removed the same field from the corresponding drain-start INFO event.
- Knowledge drift: runtime-state comments still named shared `conn()` rather than wrapper-owned `owned_conn()`, and concurrency review evidence still described the former detached cleaner as current despite retained-handle shutdown/restart support.
- Decision: `change` (simple). Remove only the INFO topology field, correct ownership comments, and distinguish historical from current cleaner lifecycle guidance.
- Preserved semantics: service lifecycle INFO, DEBUG backend detail, ten-second scan, immediate first tick, exact generation keys, wrapper-owned cleanup, retained handle, cancellation, await, token reset, restart, and production shutdown order.
- Freshness revalidation: valid at `4ad1de05c`; B0021 changed campaign records only.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0022 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: removed the backend field from fully-drained INFO, updated three comments to `owned_conn()`, and revised the cleaner shutdown review section to historical/current facts.

## Independent review

- Reviewer: `b0001_review`, independent from the B0022 analyst and implementer.
- Verdict: approved with no findings. The reviewer verified topology levels, exact-wrapper ownership, no task behavior change, retained-handle shutdown wiring, and restart coverage.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused implementation checks | passed | LeastConn 6, runtime-state 6, and backend 3 tests plus Gateway all-target check, formatting, agent-doc, no-tracing, English-only, diff, and frozen checks passed. |
| Independent review checks | passed | LeastConn, runtime-state, backend, and Pingora shutdown tests plus Gateway library check/clippy, formatting, agent-doc, no-tracing, and diff checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `5b40da9b2114cb676492f9c6d2a2cecd48a27f74`.
