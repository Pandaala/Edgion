# B0017 — LB Weighted Selector

## Scope

- Baseline: `336b7fca858dfc082c67341e18bdc2608973d03f`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/lb/backend_selector/weighted_selector.rs`
- Allowed code files: `edgion-gateway/src/lb/backend_selector/weighted_selector.rs` only.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the code file above and B0016/B0017 campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0017 analysis turn.
- Confirmed defect: the public WRR constructor summed arbitrary `usize` weights before normalization, and normalization repeated a `usize` sum. Large coprime weights could panic in debug builds or wrap in optimized builds before the existing bounded scaling protected construction.
- Decision: `change` (simple). Detect all-zero input without summing and perform post-GCD total/scaling arithmetic in `u128` before converting bounded results back to `usize`.
- Preserved semantics: all public errors and messages, GCD reduction, zero exclusion, minimum-one anti-starvation, proportional rounding, smooth tie order, 10,000-entry budget plus per-item rounding allowance, u16 indexing, and the O(1) allocation-free atomic request path.
- Freshness revalidation: valid at `336b7fca8`; B0016 changed selector input validation and facade tests but did not alter WRR internals or valid equal-length inputs.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0017 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: replaced zero detection by summation with an all-zero predicate, widened normalization totals and proportional scaling, and bounded values before casting.
- Regression: two large coprime near-maximum weights now construct without overflow, remain within the existing sequence bound, and retain both near-equal shares.

## Independent review

- Reviewer: `b0001_review`, independent from the B0017 analyst and implementer.
- Verdict: approved with no findings. The reviewer proved the widened sum/multiply bounds, cap and anti-starvation bounds, cast safety, deterministic regression value, and unchanged request path.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused implementation checks | passed | Weighted-selector 9 and selector 16 tests plus Gateway all-target check, formatting, no-tracing, English-only, diff, and frozen checks passed. |
| Independent review checks | passed | Weighted-selector, selector, TCP/TLS/UDP, HTTP, and gRPC suites plus Gateway library check/clippy, formatting, no-tracing, English-only, diff, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `ff372c88299b2992588b5f9665c2aeca40b72c34`.
