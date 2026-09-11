# B0033 — Backend Circuit Breaker

## Scope

- Baseline: `54543984449c8bcf6b2fe86c10cc979a13b950ee`
- Files: `edgion-gateway/src/backends/circuit_breaker/mod.rs`, `edgion-gateway/src/backends/circuit_breaker/concurrency_integration_tests.rs`
- Allowed support files: none beyond directly related crate-local tests already in the target pair
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the two target files; `00-context.md`; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `integration-freeze.tsv`; legacy follow-up index; this batch record; removal of the renamed `file-inventory.md`

## Context and value gate

- Analysis subagent: `b0033_analysis`
- Coverage summary: both target files, all production callers, request-context ownership, backend-traffic-policy publication, metrics, schema, crate-local tests, frozen integration coverage, and closed circuit-breaker precedents
- Key responsibility/call chain: backend policy publishes the Service limit; upstream selection acquires a `CbPermit`; request-context replacement or drop releases the endpoint counter through RAII
- Concrete opportunity and decisive evidence: the contention test intentionally leaks every winning permit with `mem::forget`, so it neither releases the owned backend handles nor proves that concurrent winners settle the counter back to zero
- Expected benefit: remove the test-process leak and strengthen concurrent RAII release coverage without changing production behavior
- Regression surface: one crate-local test; production code and frozen integration tests remain unchanged
- Decision: change

## Implementation

- Implementation subagent: `b0033_implementation`
- Complexity: simple
- Plan summary: return each winning permit from its thread, retain winners through peak assertions, drop them together, and assert zero in-flight permits
- Actual change: contention threads return `Option<CbPermit>`; the parent retains all winners through the peak assertions, drops them together, and verifies zero in-flight permits
- Behavior intentionally preserved: exactly `MAX` concurrent acquisitions succeed; winners remain live through peak assertions; production admission, endpoint isolation, metrics, and policy behavior are unchanged
- Tests added or updated: strengthened `exactly_max_permits_win_under_contention`

## Independent review

- Reviewer: `b0033_review`
- Verdict: LGTM
- Must Fix resolution: none
- Deferred observations: none; the broader mixed-generation policy publication concern remains in its existing canonical task
- Adjudication: none

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 files fingerprinted and unchanged |
| Targeted circuit-breaker tests | passed | 4 passed, 0 failed |
| `cargo fmt --all` and format check | passed | no out-of-scope Rust changes |
| Workspace compile check | passed | all targets |
| Workspace Clippy | passed | exit 0; 929 pre-existing Gateway warnings |
| SSA, metrics, Pingora source, Gateway no-tracing | passed | all guards clean |
| Agent documentation | pre-existing failure | diagnostic agent reproduced the same 13 missing external Pingora links; safe continuation confirmed |
| Frozen integration paths after review and verification | passed | no fingerprint drift |

## Handoff

- Ledger/changelog updated: yes
- Commit: `770be849`
