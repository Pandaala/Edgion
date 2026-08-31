# B0042 — Backend Outlier Detection

## Scope

- Baseline: `c3ff57b5a21f9e0efe549d8757671e27a9e42f01`
- Files: `edgion-gateway/src/backends/outlier_detection/config.rs`, `edgion-gateway/src/backends/outlier_detection/mod.rs`, `edgion-gateway/src/backends/outlier_detection/state.rs`, `edgion-gateway/src/backends/outlier_detection/store.rs`
- Allowed support files: colocated unit tests within the four targets only
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the four target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `deferred/INDEX.md`; any analyst-verified new deferred record; the B0041 and B0042 batch records

## Context and value gate

- Analysis subagent: `b0035_analysis`
- Coverage summary: all four targets, BackendTrafficPolicy normalization/publication, HTTP/TCP/TLS reporting callers, selector/panic interaction, backend/Service cleanup, AI credential reuse, counters/backoff/cooldown/half-open generations, concurrency boundaries, metrics, cache-residue coverage, and all colocated tests
- Key responsibility/call chain: policy winners publish per-Service outlier configuration; request/error hooks update per-endpoint passive-ejection state; selectors perform lock-free/serialized ejection decisions with Service percent arbitration; AI credentials reuse the generation-valid half-open state
- Concrete opportunity and decisive evidence: state/store recovery tests spend roughly 6.4 seconds on wall-clock sleeps even though they only need deadlines to be expired; deterministic test-only deadline expiry can preserve the production algorithm while removing slow/flaky scheduling dependence
- Expected benefit: substantially faster and deterministic outlier unit coverage with unchanged production behavior
- Regression surface: test-only helper and tests; production thresholds, counters, durations, backoff, leases, percent cap, metrics, selector, and AI behavior remain unchanged
- Decision: change tests locally; defer detached-generation state/metric ownership

## Implementation

- Implementation subagent: `b0035_defer_review`
- Complexity: low test-only change; high-complexity generation/metric work deferred
- Plan summary: add a minimal `#[cfg(test)]` deadline-expiry helper and replace recovery/backoff sleeps while preserving exact duration arithmetic and generation/lease assertions
- Actual change: added a test-only helper that expires active failure/explicit deadlines without changing counters, generation, or probe lease; replaced recovery/backoff sleeps with deterministic expiry and stronger ejection-history assertions
- Behavior intentionally preserved: all production outlier configuration, counters, deadlines, backoff, percent arbitration, metric emission, cleanup, selector behavior, and AI half-open behavior
- Tests added or updated: strengthened half-open failure/recovery, lazy Service recovery, and repeated-ejection/backoff tests; retained the 20 ms overlap sleep that exercises concurrent half-open exclusion

## Independent review

- Reviewer: `b0034_defer_review`
- Verdict: LGTM
- Must Fix resolution: none
- Deferred observations: D0006 is evidence-backed, non-duplicative, and requires coordinated generation ownership across publication, request reporting, recovery, and cleanup; no local drain is authorized
- Adjudication: none

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Focused outlier tests | passed | 34 passed, 0 failed; runtime 0.03 seconds after compilation |
| Format | passed | `cargo fmt --all`; `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings only |
| Repository guards | partial | SSA, metrics inventory, Pingora source, and Gateway no-tracing passed; baseline diagnosis and one retry confirmed the exact 13 pre-existing missing external Pingora links |
| Frozen integration paths after review and verification | passed | 2,681 existing paths matched the manifest |

## Handoff

- Ledger/changelog updated: yes
- Deferred finding: [`D0006`](../deferred/D0006-outlier-generation-gauge-lifecycle.md)
- Commit: pending automatic commit
