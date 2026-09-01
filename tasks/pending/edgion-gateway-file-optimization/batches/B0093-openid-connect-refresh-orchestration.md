# B0093 — OpenID Connect Refresh Orchestration

## Scope
- Initial baseline: `2aae3554`; the user-authorized concurrent baseline advanced to `142e0de0`, with every target blob unchanged
- Pipeline state: implementation and independent reviews approved; all verification passed; final combined commit review in progress
- Active anchor crate: `edgion-gateway`
- Newly covered owned files: `openid_connect/{logout.rs,request_filter_impl.rs,userinfo.rs}`
- Exact implementation files: `openid_connect/{request_filter_impl.rs,token_exchange.rs,tests.rs}`; the last two are reopened support
- Exact non-coverage support file: `Cargo.lock`; it carries the current-HEAD Pingora `2e2e6741` compile requirement and the separately scoped and reviewed D0062 security prerequisite
- Examined without change: `openid_connect/{logout.rs,userinfo.rs}`
- Forbidden files: every other OpenID Connect file, resources, runtime APIs, documentation, every existing `edgion-tests/**` path, and protected user paths

## Context and value gate
- A resolved access token can come from the generation's access-token cache while the serialized session cookie intentionally carries an empty token. The preemptive-refresh helper inspected the serialized field and could therefore refresh a far-from-expiry cached token on every request.
- A cache-miss refresh and preemptive refresh were independent branches, so one request could immediately refresh twice after either failure or a short-lived successful result.
- Logout cloned the complete session payload, including bearer credentials, across asynchronous revocation even though only the short session reference needed to survive for later cache removal.

## Hard-rule boundary
- Preserve the verification-after-refresh flow and the independent reactive refresh after a verified `Expired` result.
- Preserve singleflight timeout and stale-leeway behavior, `UnauthAction`, 401/403 challenge and delay, 502 mapping, Dye, SAN metrics, PluginLog text, response headers, and best-effort logout.
- Preserve refresh-token then access-token revocation, followed by cache removal, redirect construction, and the two clearing cookies.
- Do not add tracing, metrics, dynamic error detail, network fixtures, integration-test changes, or a fourth implementation file.

## Implementation and review
- Complexity: authentication state-machine change; implementation followed a revised, independently approved plan
- Analyst: `b0088_new_final_review`; implementer: `b0055_runtime_analysis`; plan reviewer: `b0084_plan_review`; final reviewer: `b0093_final_review`
- D0062 retained separate technical roles: analyst `b0088_new_final_review`; implementer `b0055_runtime_analysis`; plan reviewer `b0084_plan_review`; final reviewer `b0093_final_review`. A fresh combined final review approved the atomic closeout.
- The preemptive helper now consumes the request's resolved token instead of the intentionally empty serialized carrier.
- One exact cache-miss predicate both runs the initial refresh and records that it was attempted, preventing a second pre-verification refresh in the same request.
- Logout takes the ID-token hint, clones only the session reference, and moves the remaining payload into revocation.
- `Cargo.lock` pins all 16 Pingora fork packages to `2e2e6741`, the verified revision required by the already committed selected-response API consumer. The separately reviewed D0062 prerequisite also updates Wasmtime/Pulley to 46.0.3 and Cranelift to 0.133.3 without adding WASI capabilities.
- The shared lockfile support is incorporated atomically into this closeout; it does not expand the OIDC implementation boundary or count as Wasm/Pingora anchor coverage.
- Deterministic regressions cover a far-expiry cache hit, a short-lived cache-miss result, one held-flight timeout window, an empty refresh token with unavailable credentials, the helper branch table, and logout hint/cache/cookie ordering.
- Final verdict: code and tests approved with no Must Fix. D0060 owns the separate session-cookie upstream contract decision.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Static review | passed | corrected plan and independent final code/test review approved |
| Focused tests | passed | post-upgrade filters passed 106 OIDC, 50 PluginRuntime-related, and 174 request-body-related tests; the narrower 41 PluginRuntime and 5 selected-response seam sets also passed before the final lock update |
| Full Gateway library | passed | 3,549 passed and 2 ignored with a high-space temporary directory; D0061 records the pre-existing system-temp space assumption |
| Workspace and binary gates | passed | locked workspace check/Clippy and locked Gateway binary build; Clippy emitted only existing warnings |
| Repository guards | passed | format, agent docs, SSA, metrics, Pingora source, gateway no-tracing, English-only, diff, exact scope, and frozen paths |
| Frozen integration paths | passed | user-authorized baseline reconciliation was exactly 2 existing fingerprints plus 1 committed new path; no `edgion-tests/**` content was modified or staged by the campaign |
| Supply-chain guards | passed | D0062 moved Wasmtime to 46.0.3; `cargo audit` reports zero vulnerabilities and `cargo deny check` passes without a new ignore |
| Required recovery | completed | the patched lock, feature graph, full and focused tests, locked workspace/build gates, repository guards, and frozen paths were revalidated |

## Handoff
- Ledger/changelog updated: yes; 1 newly changed owned row, 2 examined-no-change rows, and 2 changed support rows revisited without double-counting coverage
- Commit: pending final combined review and exact staging
