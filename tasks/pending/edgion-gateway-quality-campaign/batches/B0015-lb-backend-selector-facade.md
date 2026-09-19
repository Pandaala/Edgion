# B0015 — LB Backend Selector Facade

## Scope

- Baseline: `7cfb8d02ec602d2e98c52e103aa51e6361cfeda9`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/lb/backend_selector/mod.rs`
- Allowed code files: none; analysis concluded examined-no-change.
- Forbidden files: every source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: B0014/B0015 campaign ledger/batch/main/changelog records only.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0015 analysis turn.
- Decision: `examined-no-change`. The module is the canonical selector facade and focused test surface; every production builder derives aligned item/weight vectors, initializes while private, and publishes the selector with its immutable route generation.
- Decisive evidence: empty, uninitialized, and all-zero inputs fail closed; single candidates avoid WRR; non-positive weights are excluded; mixed optional weights follow the documented all-or-none L4 contract; multi-candidate WRR is precomputed with a 10,000-entry cap and request selection is lock-free, allocation-free O(1).
- Rejected speculative changes: concurrent repeated initialization and manually mismatched vectors have no production path. The direct selector API is the immediately following ledger anchor and remains independently reviewable there.
- Preserved semantics: declared denied-reference traffic share, fail-closed post-selection rejection, generation pinning, HTTP/gRPC weight canonicalization, L4 all-or-none weights, and observability restraint.
- Freshness revalidation: valid at `7cfb8d02e`; B0014 changed campaign records only.

## Implementation

- Implementation: none. No material correctness, reliability, performance, security, or regression-coverage improvement justified a source edit.

## Independent review

- Reviewer: `b0001_review`, independent from the B0015 analyst.
- Verdict: approved examined-no-change with no findings after exhaustively checking HTTP, gRPC, TCP, TLS, and UDP construction and selection paths.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Analysis checks | passed | Backend-selector 14, weighted-selector 8, and TCP/TLS/UDP route-unit 5/11/5 tests passed. |
| Independent review checks | passed | Selector, route-unit, HTTP/gRPC, and common-route suites plus Gateway library check, no-tracing, diff, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
