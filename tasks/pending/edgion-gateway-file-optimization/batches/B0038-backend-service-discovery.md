# B0038 — Backend Service Discovery

## Scope

- Baseline: `353268ec2e93baafdea781c6e24f5997d4f7cdf1`
- Files: `edgion-gateway/src/backends/discovery/services/conf_handler_impl.rs`, `edgion-gateway/src/backends/discovery/services/mod.rs`, `edgion-gateway/src/backends/discovery/services/service_store.rs`
- Allowed support files: colocated unit tests within the three targets only
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the three target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; legacy follow-up index; any analyst-verified new deferred record; the B0037 and B0038 batch records

## Context and value gate

- Analysis subagent: `b0035_analysis`
- Coverage summary: all three targets, Service handler registration, serialized full/partial updates, ArcSwap publication, selector/runtime cleanup, request consumers, six colocated tests, existing delete/recreate integration coverage, and concurrency/config-sync precedent
- Key responsibility/call chain: the Service handler atomically publishes immutable Service maps, removes selector/runtime state for explicit deletions, and supplies lock-free snapshots to ordinary, ClusterIP, ExternalName, application-protocol, port-resolution, statistics, and test consumers
- Concrete opportunity and decisive evidence: reordering cleanup and publication only moves a narrow observation window because Service and EndpointSlice generations are independently synchronized; policy-store retention across Service delete/recreate is intentional and integration-covered; map or returned-Service cloning has no measured configuration-path or request-path bottleneck
- Expected benefit: no local candidate provides a correctness, performance, safety, or maintenance gain
- Regression surface: local ordering changes can alter accepted cross-kind convergence, policy cleanup would break delete/recreate semantics, and clone/store changes widen scope without evidence
- Decision: examined-no-change

## Implementation

- Implementation subagent: none
- Complexity: simple / not applicable
- Plan summary: not applicable; no implementation is authorized
- Actual change: none
- Behavior intentionally preserved: atomic Service-map publication, real incremental updates, lock-free reads, explicit-delete selector/runtime cleanup, policy retention, and accepted cross-kind convergence remain unchanged
- Tests added or updated: none

## Independent review

- Reviewer: `b0034_defer_review`
- Verdict: LGTM
- Must Fix resolution: none
- Deferred observations: no new finding; D0001 owns ExternalName cache lifecycle and D0002 owns EndpointSlice ownership transitions
- Adjudication: `b0034_defer_review` resolved the agent-doc continuation disagreement: the exact 13-link failure is pre-existing, attribution and safety are resolved, and B0038 must not absorb the absent sibling Pingora tree

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Runtime target diff | passed | all three Service discovery targets remain unchanged |
| Format | passed | `cargo fmt --all`; `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings only |
| Repository guards | partial | SSA, metrics inventory, Pingora source, and Gateway no-tracing passed; independent baseline diagnosis, adjudication, and one retry retained the exact 13 pre-existing missing external Pingora links |
| Targeted tests | not applicable | Record-only no-change batch; existing lifecycle coverage was inspected and no runtime code changed |
| Frozen integration paths after review and verification | passed | 2,681 existing paths matched the manifest |

## Handoff

- Ledger/changelog updated: yes
- Commit: `62398c46`
