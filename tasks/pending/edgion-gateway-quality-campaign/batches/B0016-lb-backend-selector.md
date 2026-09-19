# B0016 — LB Backend Selector

## Scope

- Baseline: `a319965796db76673abed452b0bc1a9b73ef6131`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/lb/backend_selector/selector.rs`
- Allowed code files: the anchor plus `edgion-gateway/src/lb/backend_selector/mod.rs` for focused regression tests.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the two code files above and B0015/B0016 campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0016 analysis turn.
- Confirmed defect: `BackendSelector::create_state` zipped item and weight vectors without first checking equal lengths. An extra item could be silently unreachable and an extra weight silently ignored, bypassing the lower WRR length-mismatch guard.
- Decision: `change` (simple). Reject both mismatch directions with the existing fail-closed selector construction error before any empty or weight-shape processing.
- Reachability: current route builders structurally produce aligned vectors, but the owning direct API violated its parallel-vector contract and silently accepted invalid input.
- Preserved semantics: equal empty inputs, equal mixed optional weights, zero/negative filtering, single/multi selection, first initialization, immutable generation pinning, all consumer error mappings, and lock-free allocation-free request selection.
- Freshness revalidation: valid at `a31996579`; B0015 changed campaign records only.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0016 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: added construction-time length equality validation and two facade regressions covering items-longer and weights-longer inputs.

## Independent review

- Reviewer: `b0001_review`, independent from the B0016 analyst and implementer.
- Verdict: approved with no findings. The reviewer confirmed the mismatch error priority, genuine empty/mixed-weight behavior, existing selection semantics, and lack of request-hot-path impact.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused implementation checks | passed | Selector 16, weighted-selector 8, route-unit 21, HTTP 11, and gRPC 8 tests plus Gateway all-target check, formatting, no-tracing, English-only, diff, and frozen checks passed. |
| Independent review checks | passed | Selector, weighted-selector, TCP/TLS/UDP, HTTP/gRPC, and common-route suites plus Gateway library check/clippy, formatting, no-tracing, English-only, diff, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
