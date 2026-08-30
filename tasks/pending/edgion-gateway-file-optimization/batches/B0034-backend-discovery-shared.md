# B0034 — Backend Discovery Shared Layer

## Scope

- Baseline: `770be84971d5cb7ab11b5d7f3ec4c6226e38d390`
- Files: `edgion-gateway/src/backends/discovery/conf_handler_common.rs`
- Allowed support files: directly related crate-local unit tests only when the analyst names their exact paths
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the target file; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `deferred/INDEX.md`; the B0033 and B0034 batch records

## Context and value gate

- Analysis subagent: `b0034_conf_handler_analysis`; the initial three-file survey was interrupted before producing a report, so its technical output is not used
- Coverage summary: target file, both EndpointSlice handler callers, store/reverse-index updates, selector-cell replacement, runtime draining cleanup, Service sibling lifecycle, directly related crate tests, and binding review precedents
- Key responsibility/call chain: EndpointSlice full or partial publication computes affected Services, materializes new backends, snapshots each selector cell, cleans removed runtime state, and atomically replaces cell backends
- Concrete opportunity and decisive evidence: `cleanup_removed_backends` finds removed addresses and then linearly searches the old snapshot for each address, producing worst-case quadratic cleanup during large endpoint churn
- Expected benefit: a first-occurrence address index reduces large-Service resync/churn cleanup from O(E×R) to O(E+R) while preserving the existing lookup result even for duplicate or missing snapshot entries
- Regression surface: one cold-path helper and focused colocated tests; no selector, runtime-state, schema, or integration-test changes
- Decision: change after adjudication

## Implementation

- Implementation subagent: `b0034_implementation`
- Complexity: moderate
- Plan summary: build a first-occurrence address index from the old snapshot, keep the unique removed-address loop and cleanup order, replace repeated scans with indexed lookup, and add focused duplicate/missing-entry tests
- Actual change: builds a first-occurrence address-to-backend index only when removals exist and replaces every removed-address snapshot scan with an indexed lookup
- Behavior intentionally preserved: selector cache invalidation, live-backend draining, circuit-breaker cleanup, and atomic cell replacement remain unchanged
- Tests added or updated: duplicate-address first-match behavior and missing-snapshot zero-inflight behavior

## Independent review

- Reviewer: `b0034_code_review`; the earlier `b0034_defer_review` classification disagreement was resolved by adjudication before implementation
- Verdict: LGTM
- Must Fix resolution: none after adjudication and final code review
- Deferred observations: none
- Adjudication: `b0034_adjudication` resolved the analyst/reviewer disagreement in favor of a local O(E+R) change; the analyst revised the value decision and implementation brief

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 files unchanged |
| Targeted discovery cleanup tests | passed | 2 passed, 0 failed |
| `cargo fmt --all` and format check | passed | no out-of-scope Rust changes |
| Workspace compile check | passed | all targets |
| Workspace Clippy | passed | exit 0; 929 pre-existing Gateway warnings |
| SSA, metrics, Pingora source, Gateway no-tracing | passed | all guards clean |
| Agent documentation | pre-existing failure | fresh diagnostic reproduced the same 13 missing external Pingora links and confirmed safe continuation |
| Frozen integration paths after review and verification | passed | no fingerprint drift |

## Handoff

- Ledger/changelog updated: yes
- Commit: pending automatic commit
