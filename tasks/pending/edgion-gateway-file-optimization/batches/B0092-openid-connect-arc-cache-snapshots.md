# B0092 — OpenID Connect Arc Cache Snapshots

## Scope
- Baseline: `dc6ab680`; all three target blobs were unchanged when the batch started
- Pipeline state: committed
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/openid_connect/{discovery_jwks.rs,jwks_provider.rs,refresh_flight_registry.rs}`
- Exact implementation files: `edgion-gateway/src/plugins/http/openid_connect/{discovery_jwks.rs,jwks_provider.rs}`
- Examined without change: `edgion-gateway/src/plugins/http/openid_connect/refresh_flight_registry.rs`
- Forbidden files: every other OpenID Connect file, resources, Controller, documentation, shared clients and runtime/session APIs, every existing `edgion-tests/**` path, and protected user paths

## Context and value gate
- Successful discovery and JWKS fetches were cached as owned values. Every fresh cache hit, stale fallback, and minimum-refresh backoff response deeply cloned the discovery endpoint strings or complete key set.
- Cache entries are immutable published snapshots, and callers only need shared read access while constructing owned downstream values.
- Decision: wrap each successful publication once in `Arc`, clone only that handle on subsequent lookups, and keep the existing synchronization and refresh state machine intact.

## Hard-rule boundary
- Preserve TTL, forced refresh, minimum-refresh backoff, double-checked locking, stale fallback, served-stale reporting, validation, response caps, error mapping, Dye behavior, locks, and singleflight semantics.
- Preserve key rotation: an in-flight reader retains its old immutable snapshot while a successful refresh atomically publishes a new snapshot.
- Do not add request-level logs, metrics, tracing, sensitive formatting, a new cache implementation, background work, or integration-test changes.
- Do not alter the refresh-flight registry; its separate token-refresh coalescing semantics remain unchanged.

## Implementation and review
- Complexity: moderate authentication cache-ownership change; implementation started only after an independent plan review and correction
- Analyst: `b0084_final_code_review`; implementer: `b0055_runtime_analysis`; plan reviewer: `b0084_plan_review`; final reviewer: `b0088_new_final_review`
- Exact change: discovery and JWKS cache slots now hold `Arc` snapshots, `JwksLookup` shares the published set, and successful fetches allocate one snapshot before publication
- Regression coverage: a deliberately non-`Clone` discovery fixture exercises seed, fetch, and cached paths; pointer-identity assertions cover fresh hits, TTL refresh, minimum-refresh backoff, stale fallback, and rotation
- Final verdict: approved with no findings; the two-file diff preserved the provider state machine and left the refresh-flight registry and integration tests untouched
- Deferred adjudication: D0059 owns cold discovery failure coalescing, cancellation, and failure backoff because those require an explicit reliability design beyond this ownership change.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 21 provider, 104 complete OIDC-module, and 41 PluginRuntime tests |
| Independent reviews | passed | corrected plan and final code reviews approved with no remaining issue |
| Full workspace gates | passed | format, all-target check, and Clippy completed with only existing warnings |
| Repository guards | passed | agent docs, English-only, SSA, metrics, Pingora source, gateway no-tracing, diff, scope, and frozen paths |
| Frozen integration paths | passed | no existing integration file changed; D0059 needs deterministic provider tests, not a new integration issue |

## Handoff
- Ledger/changelog updated: yes; 2 owned rows changed and 1 examined-no-change
- Commit: `2aae3554`
