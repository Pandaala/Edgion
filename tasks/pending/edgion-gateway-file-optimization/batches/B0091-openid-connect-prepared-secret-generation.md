# B0091 — OpenID Connect Prepared Secret Generation

## Scope
- Baseline: `7b3480d1`; all target blobs were unchanged when the batch started
- Pipeline state: committed
- Active anchor crate: `edgion-gateway`
- Newly covered owned files: `openid_connect/{callback.rs,plugin.rs,session_cookie.rs}`
- Necessary reopened support: `openid_connect/{token_exchange.rs,token_verify.rs,tests.rs}`
- Documentation support: standalone English and Chinese OpenID Connect user guides
- Exact implementation boundary: the six Gateway files and two standalone guides above
- Forbidden files: other OpenID Connect files, resources, Controller, PluginRuntime, shared Secret/TLS APIs, every existing `edgion-tests/**` path, and protected user paths

## Context and value gate
- A standard Controller-published generation retained four raw resolved Secret/TLS carrier fields after building its HTTP client.
- Session flows repeatedly loaded or cloned the session secret, hashed it, and constructed a new AES-256-GCM cipher; OAuth calls repeatedly cloned the client secret.
- Legacy direct construction also performed global SecretStore reads on request paths, and dynamic lookup or parsing detail could enter public authentication responses.
- Decision: prepare one immutable credential/cipher snapshot per generation, consume raw carriers, and use fixed authentication failures while preserving the configured OIDC policy.

## Hard-rule boundary
- A non-empty resolved credential is authoritative; only absent or defensively empty resolved values may use the existing legacy SecretStore fallback, once during construction.
- Preserve namespace and Secret-key precedence, TLS resolution, client construction, callback/token/refresh/introspection/revoke flow order, cookies, caches, `unauthAction`, failure delay, challenges, and best-effort logout/cookie persistence.
- Authentication credential failures use fixed delayed generic 401 responses and static PluginLog categories. No Secret identity, key, encoding, length, token, cookie, or claim detail may reach the response or log.
- Add no tracing or metrics. Do not implement D0056/D0057 or edit existing integration tests.

## Implementation and review
- Complexity: complex; implementation started only after a corrected independent plan review
- Analyst: `b0084_final_code_review`; implementer: `b0055_runtime_analysis`; plan reviewer: `b0084_plan_review`; final reviewer: `b0088_new_final_review`
- Prepared state: retain one opaque client-secret value and one AES-256-GCM session cipher, with a payload-free unavailable state and no sensitive `Debug` or `Clone`
- Ownership: clear the runtime config's four resolved carriers, move CA/mTLS material into canonical client construction, remove retained plugin namespace, and leave the caller's config untouched
- Hot path: borrow the prepared client secret only while the request builder owns its auth data; synchronously borrow the prepared cipher for cookie operations; never carry either borrow across an await
- Failure behavior: missing, empty, non-UTF-8, or short required authentication material maps to the fixed generic 401 path; IdP transport and serialization retain their separate dependency mapping; explicit Pass/Deny/Auth and best-effort revoke/cookie-update behavior remain intact
- Regression coverage: static name, caller/runtime carrier ownership, resolved-versus-store precedence, exact-subprocess store isolation, invalid material, stable client-secret reference, K1/K2 generation and cookie isolation, `Send + Sync`, and exact delayed 401 header/body/shutdown/log ordering
- Documentation: aligned both languages on generation publication, rotation, old-cookie invalidation, mode-dependent Secret requirements, detail-free credential failures, and the separate TLS-construction boundary without claiming zeroization
- Final verdict: approved after narrowing the guides to the mode-dependent credential contract and existing endpoint-specific dependency-error mapping
- Deferred adjudication: D0058 owns the baseline OIDC dependency-error response redaction. This batch requires no new integration issue.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 103 complete OIDC-module and 41 PluginRuntime tests |
| Independent reviews | passed | corrected plan approved before implementation; final documentation correction approved |
| Full workspace gates | passed | format, all-target check, and Clippy completed with only existing warnings |
| Repository guards | passed | agent docs, English-only, SSA, metrics, Pingora source, gateway no-tracing, diff, scope, and frozen paths |
| Frozen integration paths | passed | no existing integration file changed and no new integration issue was needed |

## Handoff
- Ledger/changelog updated: yes; 3 newly changed owned rows and 3 changed support rows revisited without double-counting coverage
- Commit: `dc6ab680`
