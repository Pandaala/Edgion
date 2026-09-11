# B0039 — Gateway Backend TLS Store

## Scope

- Baseline: `62398c46786edc4a0e4043672d940c9c6c99d75f`
- Files: `edgion-gateway/src/backends/gateway_backend_tls_store.rs`
- Allowed support files: colocated unit tests in the target file only
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the target file; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; legacy follow-up index; any analyst-verified new deferred record; the B0038 and B0039 batch records

## Context and value gate

- Analysis subagent: `b0035_analysis`
- Coverage summary: complete target, runtime and peer-builder consumers, admin statistics, target/peer tests, Controller-resolved certificate publication, Gateway full/partial synchronization, BackendTLSPolicy gating/precedence, BoringSSL parsing, snapshot concurrency, secret boundaries, and review precedent
- Key responsibility/call chain: the store publishes lock-free immutable Gateway-key to client-certificate Secret snapshots; Gateway configuration updates mutate it and BoringSSL peer construction consults it only after BackendTLSPolicy enables TLS and supplies no policy identity
- Concrete opportunity and decisive evidence: authoritative `GatewayHandler::full_set` clears the primary Gateway store but invokes the TLS updater only for Gateways present in the new snapshot; omitted keys therefore retain stale Secret material and inflate the TLS-store count
- Expected benefit: authoritative replacement would retire omitted identities and align secret lifetime/statistics with the Gateway generation
- Regression surface: a complete fix spans the store replacement API and `GatewayHandler::full_set`, while preserving incremental updates, existing `Arc<Secret>` readers, policy identity precedence, policy-required TLS gating, and BoringSSL fail-close behavior
- Decision: defer

## Implementation

- Implementation subagent: none; the verified lifecycle issue exceeds the frozen one-file boundary
- Complexity: medium, cross-module lifecycle change
- Plan summary: not applicable; no target-only replacement API or partial fix is authorized
- Actual change: none
- Behavior intentionally preserved: lock-free reads, serialized production mutation, BackendTLSPolicy gating, identity precedence, BoringSSL parsing/fail-close behavior, redaction, and admin count semantics remain unchanged pending a coordinated fix
- Tests added or updated: none

## Independent review

- Reviewer: `b0034_defer_review`
- Verdict: LGTM
- Must Fix resolution: none
- Deferred observations: D0003 is evidence-backed, non-duplicative, and correctly spans authoritative replacement in this store plus `GatewayHandler::full_set`; no target-only partial API is justified
- Adjudication: none

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Runtime target diff | passed | `gateway_backend_tls_store.rs` remains unchanged |
| Format | passed | `cargo fmt --all`; `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings only |
| Repository guards | partial | SSA, metrics inventory, Pingora source, and Gateway no-tracing passed; independent baseline diagnosis and one retry confirmed the exact 13 pre-existing missing external Pingora links |
| Targeted tests | not applicable | Record-only defer; no runtime code changed |
| Frozen integration paths after review and verification | passed | 2,681 existing paths matched the manifest |

## Handoff

- Ledger/changelog updated: yes
- Deferred finding: `D0003`
- Commit: `abca60f3`
