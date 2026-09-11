# B0043 — Backend Peer Builder

## Scope

- Baseline: `803d934275e1559a6a8a08b0209aba042eea4e08`
- Files: `edgion-gateway/src/backends/peer_builder.rs`
- Allowed support files: colocated unit tests in the target file only
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the target file; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; legacy follow-up index; any analyst-verified new deferred record; the B0042 and B0043 batch records

## Context and value gate

- Analysis subagent: `b0035_analysis`
- Coverage summary: reviewed the complete target, every peer-construction caller, Service `appProtocol` handling, BackendTLSPolicy TLS/SNI/CA/client-identity/SAN/pool isolation, Gateway identity fallback, address safety, colocated tests, and relevant review precedent; unsupported Gateway rustls parity was excluded.
- Key responsibility/call chain: typed backend selection proof flows through Service, DirectEndpoint, or ExternalName address selection and port resolution, then shared address safety, TLS peer construction, route-specific HTTP version/ALPN adjustment, and Pingora pooling.
- Concrete opportunity and decisive evidence: client identity parses `tls.crt` with `X509::from_pem`, so a standard leaf-plus-intermediate Secret sends only the first certificate upstream. The helper also carries an impossible `Ok(None)` state, `appProtocol` lowercasing allocates per peer selection, and exclusive TLS identity branches clone owned SNI unnecessarily.
- Expected benefit: preserve the complete PEM-ordered mTLS client certificate chain and remove small request-path allocations without changing routing or TLS policy semantics.
- Regression surface: single-certificate identity, malformed or missing Secret data, policy-over-Gateway identity precedence, case-insensitive protocol aliases, SNI, SAN validation, CA handling, pool grouping, gRPC HTTP/2 selection, address safety, and port fallback.
- Decision: change; medium complexity.

## Implementation

- Implementation subagent: `b0035_defer_review`
- Complexity: medium
- Plan summary: parse the complete client certificate stack and reject an empty stack, simplify the helper result type, consume owned SNI in exclusive branches, replace allocating protocol normalization with case-insensitive comparisons, and add colocated supported-BoringSSL regression tests.
- Actual change: `tls.crt` now uses `X509::stack_from_pem`, rejects an empty certificate stack, preserves PEM order, and supplies the complete vector to `CertKey::new`; the impossible optional success state and two exclusive-branch SNI clones were removed; `appProtocol` aliases now use allocation-free ASCII-case-insensitive comparisons.
- Behavior intentionally preserved: BackendTLSPolicy remains the sole TLS-enablement proof; policy identity still wins over Gateway identity; missing or malformed certificate/key data remains fail-close; CA, SAN, pool hashing, ALPN, address safety, and port fallback are unchanged.
- Tests added or updated: colocated tests cover a single certificate, an ordered leaf-plus-intermediate chain, missing/empty/malformed identity material, all existing mixed-case H2C/WS/WSS aliases, and an unknown alias; the existing identity-source matrix covers policy precedence.

## Independent review

- Reviewer: `b0034_defer_review`
- Verdict: LGTM; no Must Fix or Should Fix findings.
- Must Fix resolution: none required.
- Deferred observations: invalid applicable BackendTLSPolicy/plaintext downgrade later landed as the Gateway `BackendTlsPolicyLookup` contract (`skills/01-architecture/02-gateway/07-backends.md`); full-snapshot partial publication is covered by standalone follow-up task; request-path port resolution and unsafe fallback are covered by standalone follow-up task.
- Adjudication: no new campaign finding; the broader observations are duplicates of existing, wider tasks and B0043 remains target-local.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Format | passed | `cargo fmt --all` |
| Focused tests | passed | `cargo test -p edgion-gateway peer_builder`: 9 passed, 0 failed |
| Workspace check | passed | `cargo check --workspace --all-targets` |
| Workspace Clippy | passed | `cargo clippy --workspace --all-targets`; repository-existing warnings only |
| SSA force guard | passed | 1,430 files checked; 1 allowlisted |
| Metrics inventory guard | passed | 103 metrics in sync |
| Pingora source policy | passed | 16 fork packages share `edgion_v3` commit `c50f93c0bf7adee4b1ed10a0696810968f042858` |
| Gateway tracing guard | passed | data-plane boundary and cold-function registry clean |
| Frozen integration paths after implementation | passed | no existing `edgion-tests/**` path changed |
| Agent-doc consistency | known external failure | independent baseline attribution and one retry confirmed the exact 13 pre-existing missing external Pingora review links; B0043 changed none of their source files or the validator |

## Handoff

- Ledger/changelog updated: yes
- Commit: `05752539`
