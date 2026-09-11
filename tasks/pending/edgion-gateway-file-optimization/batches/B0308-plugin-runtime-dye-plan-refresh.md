# B0308 — Plugin Runtime Dye-plan Refresh

## Scope

- Baseline: `bfb8fe89`.
- Pipeline state: terminal candidate; examined-no-change after fresh independent review at `bfb8fe89`.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/plugins/runtime/dye_plan.rs`.
- Exact technical write scope is not approved during analysis. Existing `edgion-tests/**`, campaign records, unrelated modules and user-protected paths are read-only.

## Context and value gate

- External `bea47373` resolved D0083 by adding a producer/capability inventory guard and refreshing the entry-level Dye design reference. Prior B0122 evidence predates this change.
- The analyst must inspect Dye schema capability ownership, compilation, request/response rendering, producer coverage across every listed plugin and failure-policy branch, test-source scanning robustness, access-log/observability ownership, bounded hot-path behavior and deterministic tests before deciding change, examined-no-change or defer.
- Revalidate the maintained Dye-stage parity knowledge and named-jump/plugin-store interactions after B0301. Existing integration tests remain frozen.

## Analysis and review

- Analyst `b0308_dye_plan_analysis` traced shared stage-aware capabilities, plan compilation, every producer/failure-policy branch, session rendering, header-commit ownership, B0301 policy/LKG behavior and frozen fixtures. Decision: examined-no-change.
- Reviewer `b0308_dye_plan_review`: LGTM with no Must Fix, Should Fix or deferred finding. Immutable Arc-owned plans, compiled templates/conditions, fail-closed validation disagreement, anti-spoofing, queued/direct response rendering and post-commit body/trailer boundaries remain coherent.
- The regex/`include_str!` inventory is intentionally a drift smoke guard and can miss indirect producers or stage relocation. Maintained knowledge states those limits, while shared resource matrices, publication/LKG tests, executable producer tests, renderer tests and frozen named-jump fixtures establish current behavior.

## Verification

- Fresh resource Dye tests passed 27/27, Gateway Dye 21/21 and `record_outcome` 64/64. All listed producer suites and ExtProc streamed/header-commit suites passed during independent review.
- Locked Gateway all-target check, Gateway no-tracing, diff and frozen-path checks passed. No integration test was run or modified.
- Applied format plus locked workspace all-target check and Clippy passed. Agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards all passed; Clippy retained only existing warnings.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic local commit.
