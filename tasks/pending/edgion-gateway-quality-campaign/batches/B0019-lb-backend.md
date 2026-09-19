# B0019 — LB Backend Runtime Counters

## Scope

- Baseline: `46d0b33fafb1de4080506d2450a2b6de05c5e776`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/lb/backend.rs`
- Allowed code files: `edgion-gateway/src/lb/backend.rs` only.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the code file above and B0018/B0019 campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0019 analysis turn.
- Confirmed defect: replacement wrappers for one socket intentionally share scoring counters while drain ownership remains wrapper-local. A stale wrapper with zero owned connections could still decrement the shared counter and steal a live replacement wrapper's count.
- Decision: `change` (simple). Consume exact-wrapper ownership first and decrement the shared connection count only when that local decrement succeeds.
- Preserved semantics: Relaxed lock-free atomics, normal connection balance, shared transport/EWMA/circuit-breaker state, generation-local drain ownership, score and selection behavior, and request-held wrapper pinning.
- Freshness revalidation: valid at `46d0b33fa`; B0018 changed review knowledge and campaign records only.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0019 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: the saturating CAS reports success; `dec_conn` uses successful owned-count release as the authorization for shared-count release.
- Regression: two wrappers share runtime counters, the stale non-owner cannot reduce replacement traffic, the legitimate owner reaches zero, and repeated release remains saturated.

## Independent review

- Reviewer: `b0001_review`, independent from the B0019 analyst and implementer.
- Verdict: approved with no findings. The reviewer confirmed contention behavior, normal lifecycle balance, request-Arc pinning across replacement, Relaxed ordering, and distinct shared versus drain-owned consumers.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused implementation checks | passed | Backend 3, attempt 7, LeastConn 6, and circuit-breaker 5 tests plus Gateway all-target check, formatting, no-tracing, English-only, diff, and frozen checks passed. |
| Independent review checks | passed | Backend, attempt, LeastConn, circuit-breaker, coordinator, runtime-state, and selector suites plus Gateway library check/clippy, formatting, diff, and scope checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
