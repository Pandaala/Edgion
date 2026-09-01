# B0094 — OpenID Connect Validation, Error, and Module Boundary

## Scope
- Baseline: `7f82b19b`
- Pipeline state: terminal; independently approved and fully verified
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/openid_connect/{config_validation.rs,error.rs,mod.rs}`
- Exact implementation files: `edgion-gateway/src/plugins/http/openid_connect/{config_validation.rs,tests.rs}`; `tests.rs` is reopened support
- Forbidden files: every other OpenID Connect file, resources, Controller, documentation, shared clients and runtime/session APIs, every existing `edgion-tests/**` path, and protected user paths

## Analysis boundary
- Read all three files completely and inspect their direct callers, callees, colocated tests, configuration schema, plugin construction path, error-to-wire/log mapping, and applicable architecture and review knowledge.
- Require concrete correctness, ownership, duplication, or bounded-efficiency value; cosmetic re-export, naming, formatting, or error-churn changes are rejected.
- Preserve authentication failure policy, fixed public error bodies, sensitive-data redaction, PluginLog/access-log ownership, validation parity, hot reload, and the no-tracing data-plane boundary.
- Existing integration tests remain frozen. Record a separate issue only if a genuine future integration gap cannot be proven with colocated deterministic coverage.

## Handoff
- Analyst: `b0055_runtime_analysis`
- Decision: change `config_validation.rs`; defer `error.rs` to existing D0058; examine `mod.rs` without change
- Value: borrow claim scope strings directly instead of allocating one owned `String` per available scope, and remove the temporary issuer vector from introspection claim validation while preserving the exact configured-empty/configured-nonempty/unset semantics
- Complexity: simple; no plan-first gate required
- Implementer: `b0084_plan_review`; final reviewer: `b0093_final_review`
- Final review: approved with no Must Fix or Should Fix; the two-file implementation preserves scope union/deduplication, issuer tri-state, validation order, status/body/challenge/delay, PluginLog, and D0058 ownership
- Integration-test impact: no new issue; deterministic unit tests fully cover this borrow-only optimization and every existing `edgion-tests/**` path remained frozen

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 109 complete OIDC and 41 PluginRuntime tests |
| Independent review | passed | reviewer independently reran the 6 focused validation tests and approved the exact diff |
| Workspace and binary gates | passed | locked workspace check/Clippy and locked Gateway binary build; Clippy emitted only existing warnings |
| Repository guards | passed | format, agent docs, SSA, metrics, Pingora source, gateway no-tracing, English-only, diff, exact scope, and frozen paths |
| Frozen integration paths | passed | no integration file changed; no new integration issue is required |

## Coverage outcome
- Changed: `config_validation.rs`
- Examined without change: `mod.rs`
- Deferred without duplicate task: `error.rs` remains owned by D0058
- Reopened support: `tests.rs`, without double-counting coverage
- Commit: pending
