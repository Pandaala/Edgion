# B0014 — LB Attempt Accounting

## Scope

- Baseline: `b350cc6f4cd320f947b2db7fccc64bc51c91cece`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/lb/attempt.rs`
- Allowed code files: none; analysis concluded examined-no-change.
- Forbidden files: every source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: B0013/B0014 campaign ledger/batch/main/changelog records only.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0014 analysis turn.
- Decision: `examined-no-change`. `LbCommitGuard` already pins the exact selected backend wrapper generation and balances outstanding load once across successful connection, retry transitions, terminal logging, and cancellation.
- Decisive evidence: commit occurs only after connection and SAN validation; retry transitions finalize the preceding guard; terminal cleanup finalizes the last; `take`, a live latch, and `Drop` make explicit and implicit cleanup idempotent; the owned backend wrapper preserves hot-reload generation accuracy.
- Preserved semantics: LeastConn/EWMA load accounting, separate response-header EWMA latency sampling, direct-endpoint bypass, HTTP/gRPC shared callbacks, captured retry policy, observability restraint, and allocation-free lock-free hot-path behavior.
- Freshness revalidation: valid at `b350cc6f4`; B0013 changed only an exact context-size test assertion and campaign records.

## Implementation

- Implementation: none. No material correctness, reliability, performance, security, or regression-coverage improvement justified a source edit.

## Independent review

- Reviewer: `b0001_review`, independent from the B0014 analyst.
- Verdict: approved examined-no-change with no findings after checking connect/SAN failure, cancellation, duplicate cleanup, retry replacement, policy generation, direct endpoint, EWMA sampling, and overflow boundaries.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Analysis checks | passed | Attempt 7, logging 9, response sampling 5, captured-policy 1, and context-layout 1 tests passed. |
| Independent review checks | passed | Attempt, logging, response sampling, connection/proxy failure, upstream peer, backend, and sample suites plus Gateway library check, no-tracing, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `7cfb8d02ec602d2e98c52e103aa51e6361cfeda9`.
