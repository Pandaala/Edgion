# B0052 — LinkSys Redis Provider

## Scope
- Baseline: `f31d0169da977853723c3d3e63a7ea5bf332e9ee`
- Pipeline state: verified
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/link_sys/providers/redis/{client.rs,config_mapping.rs,mod.rs,ops.rs}`
- Allowed support files: colocated unit tests in the four target files only; no cross-crate support is authorized before analysis proves exact necessity
- Forbidden files: every existing `edgion-tests/**` path, other providers, LinkSys runtime/template files, shared Redis core/schema files without exact necessity evidence, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact commit allow-list: the four target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; the B0051 and B0052 batch records; any verified new deferred record and `deferred/INDEX.md`

## Context and value gate
- Analysis subagent: `b0052_analysis`
- Coverage summary: reviewed all four provider files, resource schema, runtime store/publication, Fred implementation, every production/admin consumer, shared rate limiter, sibling providers, unit/integration coverage, and binding Redis/LinkSys precedents.
- Concrete opportunity: endpoint parsing strips `rediss://`, but Fred receives a TLS connector only when the explicit TLS block is enabled; documented scheme-only TLS endpoints therefore become plaintext and fail against TLS-only Redis services.
- Expected benefit: make transport security follow the endpoint scheme consistently while retaining the canonical outbound TLS resolver and all existing explicit TLS behavior.
- Regression surface: cold-path Fred configuration for standalone, cluster, and Sentinel endpoints; no request-path or schema change.
- Decision: change `config_mapping.rs`; the other three files are expected examined-no-change. Wider parser/schema/egress/health/integration issues require separate deferred records.

## Implementation
- Implementation subagent: `b0052_implementation`
- Complexity: moderate; one configuration file, multiple topology shapes, and TLS/security invariants.
- Plan summary: derive effective transport TLS from either `rediss://` or explicit enablement, keep resolution through `resolve_outbound_tls`, and add network-free mapping regressions for all boundary cases.
- Actual change: inferred TLS from the active topology's schemes, applied explicit TLS to the whole Fred config, rejected mixed active transports without explicit TLS, ignored disabled custom TLS policy, and made Sentinel parsing scheme-aware.
- Behavior intentionally preserved: canonical outbound TLS resolution, `verify: false` fail-close, authentication, database/timeouts/reconnect/pool settings, atomic runtime publication, and request-path RedisLinkClient use.
- Tests added or updated: 29 network-free mapping tests cover standalone, cluster, Sentinel, implicit/explicit/disabled TLS, mixed transports, verification failure, and exact subprocess isolation; existing integration tests remain frozen.

## Independent review
- Reviewer: `b0052_independent_review`
- Verdict: LGTM after correcting topology-exact endpoint inference, mixed transport handling, and disabled-policy semantics.
- Must Fix resolution: standalone now uses only its consumed first endpoint; cluster/Sentinel inspect all active endpoints; mixed transports fail without explicit TLS; disabled policy cannot inject CA/mTLS settings.
- Deferred observations: [`D0013`](../deferred/D0013-redis-endpoint-contract.md), [`D0014`](../deferred/D0014-redis-configuration-contract.md), [`D0015`](../deferred/D0015-redis-dns-egress-policy.md), [`D0016`](../deferred/D0016-redis-pool-health-logging.md), [`D0017`](../deferred/D0017-redis-tls-integration-coverage.md), and [`D0018`](../deferred/D0018-disabled-tls-publication-readiness.md).
- Adjudication: none; two test diagnostics attributed cross-crate guard visibility and child Rustls provider initialization to batch test setup and supplied in-scope corrections.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Focused unit tests | passed | 29 Redis configuration tests passed after subprocess and CryptoProvider diagnostic corrections |
| Independent static review | passed | Final review approved topology, TLS, failure, redaction, isolation, and scope behavior with no remaining Must Fix |
| Format | passed | `cargo fmt --all` and final `cargo fmt --all -- --check` passed |
| Workspace compile and lint | passed | `cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets` exited zero |
| Repository guards | passed | Agent docs, SSA force, metrics inventory, Pingora source policy, and Gateway no-tracing checks passed |
| Frozen integration paths after verification | passed | 2,681 existing paths still matched the manifest; D0017 records the future TLS integration-only update |

## Handoff
- Ledger/changelog updated: yes
- Commit: `9f2c442c`
