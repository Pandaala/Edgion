# B0013 — Gateway Request Context

## Scope

- Baseline: `936032962e3e46e90a0cdf9936f5cd415ab804db`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/ctx.rs`
- Allowed code files: `edgion-gateway/src/ctx.rs` only.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the code file above and B0012/B0013 campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0013 analysis turn.
- Confirmed defect: the exact 64-bit context-layout regression still expected 1,232 bytes after the accepted HTTP and gRPC generation handles each moved from a thin `Arc<Vec<T>>` to a 40-byte slice-backed handle. The actual context is 1,248 bytes, while the independent 1,280-byte budget and allocation behavior remain valid.
- Decision: `change` (simple). Synchronize the exact expected size without changing production code or weakening the independent size and zero-allocation assertions.
- Preserved semantics: context construction/lifecycle, backend retry state, DirectPeerIp/RemoteIp identity, request-body policy, access-log and error ownership, masking/redaction, schema and TLS behavior.
- Freshness revalidation: valid at `936032962`; B0012 changed EGC DNS snapshot support and backend resolver consumption only.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0013 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: updated the 64-bit exact context-size assertion from 1,232 to 1,248 bytes. No production code, comment, budget, or allocation assertion changed.

## Independent review

- Reviewer: `b0001_review`, independent from the B0013 analyst and implementer.
- Verdict: approved with no findings. The reviewer tied the exact 16-byte increase to two accepted 8-byte slice-handle deltas and confirmed the context remains 32 bytes below budget with zero construction allocations.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused implementation checks | passed | Full context suite passed 94 tests with one intentional benchmark ignored; focused HTTP and gRPC layout tests, Gateway all-target check, formatting, no-tracing, English-only, agent-doc, diff, and frozen checks passed. |
| Independent review checks | passed | Full context and focused backend layout/allocation tests, Gateway library check, formatting, diff, no-tracing, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
