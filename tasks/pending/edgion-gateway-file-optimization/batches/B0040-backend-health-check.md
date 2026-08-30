# B0040 — Backend Health Check

## Scope

- Baseline: `abca60f3a8d91c5ae6f1e7e32f7f8eae7040a78e`
- Files: `edgion-gateway/src/backends/health/check/config_store.rs`, `edgion-gateway/src/backends/health/check/manager.rs`, `edgion-gateway/src/backends/health/check/mod.rs`, `edgion-gateway/src/backends/health/check/probes.rs`, `edgion-gateway/src/backends/health/check/status_store.rs`, `edgion-gateway/src/backends/health/mod.rs`
- Allowed support files: colocated unit tests within the six targets only
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the six target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `deferred/INDEX.md`; any analyst-verified new deferred record; the B0039 and B0040 batch records

## Context and value gate

- Analysis subagent: `b0035_analysis`
- Coverage summary: all six targets, effective BackendTrafficPolicy publication, task replacement/cancellation, jitter/tickers/semaphore, EndpointSlice snapshots, HTTP/TCP/gRPC probes, per-Service threshold/status publication, selector hot-path reads, cache-residue exposure, adjacent tests, and concurrency/SSRF precedent
- Key responsibility/call chain: effective Service policy publishes health configuration, one Tokio loop per Service probes current EndpointSlice backends under a global semaphore, status transitions publish to a lock-free selector read path
- Concrete opportunity and decisive evidence: HTTP probes follow up to ten redirects without per-hop egress classification, and manager logging formats `ProbeError` with `Debug`, which can expose remote-controlled tonic status text or configured URLs
- Expected benefit: deny unclassified redirect egress and keep probe failure logs bounded to stable static classes
- Regression surface: 3xx responses stop redirecting and are judged directly against `expectedStatuses`; logs retain attribution and failure class while dropping dependency/server detail; TCP/gRPC timeouts, thresholds, concurrency, selector behavior, and store publication stay unchanged
- Decision: change locally; defer task/status generation ownership and active health-check TLS contract

## Implementation

- Implementation subagent: `b0035_defer_review`
- Complexity: low local changes; high-complexity lifecycle/TLS work deferred
- Plan summary: deny HTTP redirects through the canonical outbound-client policy, classify every `ProbeError` with a fixed static label, log only that label, and add focused colocated regressions
- Actual change: HTTP probes now deny redirects; `ProbeError::class` maps all nine variants to fixed static labels; manager logs use only those labels
- Behavior intentionally preserved: proxy disabling, timeouts, Host override, expected-status handling, TCP/gRPC semantics, thresholds, semaphore behavior, selector reads, and store publication
- Tests added or updated: colocated redirect test proves a 302 target is not contacted; tonic sentinel test proves the status message cannot enter the error class

## Independent review

- Reviewer: `b0034_defer_review`
- Verdict: LGTM
- Must Fix resolution: none
- Deferred observations: D0004 and D0005 are evidence-backed, non-duplicative, and must remain separate; neither receives a partial lifecycle, schema, or TLS implementation
- Adjudication: none

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Focused health-check tests | passed | 32 passed, 0 failed |
| Format | passed | `cargo fmt --all`; `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings only |
| Repository guards | partial | SSA, metrics inventory, Pingora source, and Gateway no-tracing passed; baseline diagnosis and one retry confirmed the exact 13 pre-existing missing external Pingora links |
| Frozen integration paths after review and verification | passed | 2,681 existing paths matched the manifest |

## Handoff

- Ledger/changelog updated: yes
- Deferred findings: [`D0004`](../deferred/D0004-health-task-status-generation-ownership.md), [`D0005`](../deferred/D0005-active-health-check-tls-contract.md)
- Commit: `31d8bcdd`
