# B0070 — CSRF Expiry and Runtime Ownership

## Scope
- Baseline: `71312d4e`
- Pipeline state: independently reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned anchor files: `edgion-gateway/src/plugins/http/csrf/{mod.rs,plugin.rs,token.rs}`
- Exact supporting allow-list: `edgion-resources/src/resources/edgion_plugins/plugin_configs/csrf.rs`; `docs/en/user-guide/http-route/filters/edgion-plugins/csrf.md`; `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/csrf.md`
- Allowed tests: colocated tests in the four Rust implementation/config files only
- Forbidden files: every existing `edgion-tests/**` path, PluginRuntime and PluginStore, session trait/adapter, Controller resolver, CRDs, Cargo manifests, metrics/access-log code, other plugins/resources/docs, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/csrf/mod.rs`; `edgion-gateway/src/plugins/http/csrf/plugin.rs`; `edgion-gateway/src/plugins/http/csrf/token.rs`; `edgion-resources/src/resources/edgion_plugins/plugin_configs/csrf.rs`; `docs/en/user-guide/http-route/filters/edgion-plugins/csrf.md`; `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/csrf.md`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0069-credential-injector-sensitive-buffer-transfer.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0070-csrf-expiry-runtime-ownership.md`; `tasks/pending/edgion-gateway-file-optimization/deferred/D0041-csrf-duration-string-migration.md`; `tasks/pending/edgion-gateway-file-optimization/deferred/D0042-csrf-secret-rotation-integration-coverage.md`; `tasks/pending/edgion-gateway-file-optimization/deferred/INDEX.md`; `tasks/todo/migrate-csrf-expires-to-duration-string.md`; and `tasks/todo/add-csrf-secret-rotation-integration-coverage.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `71312d4e`; the three anchor blobs, resource config, Controller resolver, runtime construction, session cookie API, PluginStore publication, docs, and frozen integration scenarios remain unchanged.
- Baseline: 6 Gateway CSRF tests, 4 resource CSRF tests, 1 Gateway resolved-key validation test, and 1 canonical session-cookie helper test pass.
- Runtime ownership: `Csrf` retains a full cloned config containing the resolved signing key and also clones the key separately. Request serving needs only the token name, positive lifetime, secure-cookie flag, and one key generation.
- Canonical reuse: the plugin reparses the Cookie header locally even though `PluginSession::get_cookie` is the shared cookie lookup boundary.
- Expiry defect: nonpositive `expires` disables server-side expiry, and direct subtraction against an attacker-controlled decoded timestamp can overflow. Cookie Max-Age is not a server-side replay limit.
- Schema gap: shared validation does not require positive expiry or a bounded HTTP-token name; the existing `gwapi_types::{is_valid_header_token, HEADER_NAME_MAX_LEN}` helper can cover both HTTP field-name and RFC 6265 cookie-name token grammar. The literal `cookie` name needs a narrow case-insensitive rejection because the same configured name is used as both a Cookie member and request header.
- Decision: narrow runtime state and key ownership, use the canonical cookie accessor, validate configuration before publication, and make token expiry arithmetic deterministic and fail closed without changing the token wire/HMAC shape.

## Hard-rule boundary
- Preserve the standard Base64 JSON wire shape, serialized field names, 32-byte CSPRNG nonce, baseline HMAC-SHA256 construction and signing input, signature comparison, and all closed CSRF crypto precedents. Do not re-report or redesign P0-04, P0-04b, or P0-09.
- Preserve GET/HEAD/OPTIONS safe-method behavior, request-stage cookie queuing, append-only `Set-Cookie`, `cookieSecure` default and override, SameSite=Lax, Path=/, HttpOnly=false, unsafe-request 401 status, exact response bodies, static PluginLog entries, unresolved-Secret 500 sealing, conditions, ReferenceGrant resolution, and immutable hot-reload generations.
- Use the shared header-token validator; add no plugin-local cookie-token parser or broad browser forbidden-header policy.
- Add no tracing, metrics, access-log fields, dependencies, compatibility aliases, CRD changes, or existing integration-test edits.
- This batch may validate positive integer seconds defensively but must not disguise the separate GEP-2257 duration-string migration.

## Deferred candidates
- Migrate operator-facing `expires` from integer seconds to the canonical GEP-2257 duration string in a future cross-layer change that may update integration fixtures after this frozen campaign.
- Add a future standalone integration scenario for CSRF Secret rotation: old-token rejection, new-token success, Secret deletion fail-close, and recovery. Do not edit current `edgion-tests` in B0070.

## Plan-first implementation
- Complexity: complex; signing-key ownership, attacker-controlled time arithmetic, token wire/crypto invariants, shared validation, and cross-crate/docs scope
- Implementer: `b0055_runtime_analysis`, which returned a plan without editing and is distinct from the analyst and reviewers
- Plan reviewer: `b0063_fresh_code_review`, independent from the analyst and implementer
- Approved plan: narrow the immutable runtime state and key generation; use the canonical cookie accessor with padded-Base64 regression; make decode errors nonallocating; verify positive TTL with one time sample, explicit pre-epoch rejection, checked arithmetic, future rejection, and inclusive expiry; reuse shared HTTP-token validation with a narrow `cookie` conflict; update exact EN/ZH constraints; preserve all wire, crypto, cookie, status, log, and hot-reload contracts.
- Deferred plan: D0041 is `needs-discussion` and must adjudicate the closed seven-field GEP-2257 precedent before any migration. D0042 is `needs-implementation`, depends on B0070 but not D0041, and may later add a standalone dynamic scenario plus only its minimal wiring without rewriting existing CSRF integration files.
- Material deviations require a revised independent plan review.

## Independent review
- Final code reviewer: `b0070_final_code_review`, fresh and distinct from the plan reviewer, analyst, and implementer
- Code verdict: approved with no technical findings after verifying scope, runtime key ownership, canonical padded-Base64 Cookie lookup, wire/HMAC stability, fail-closed checked expiry, shared resource validation, exact response/log/Cookie behavior, and EN/ZH documentation. The requested HMAC terminology correction was record-only.
- Deferred adjudication: D0041 is `needs-discussion` and must adjudicate the closed seven-field GEP-2257 precedent before any migration. D0042 is `needs-implementation`, depends on B0070 but not D0041, and adds a standalone dynamic Secret-rotation scenario after the frozen campaign without rewriting the existing CSRF scenario.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline focused tests | passed | 6 Gateway CSRF, 4 resource CSRF, 1 resolved-key gate, and 1 session-cookie helper test |
| Implementation and focused tests | passed | 16 Gateway CSRF tests, 7 resource tests, 1 resolved-key sealing test, and 1 canonical cookie-helper test; Gateway suite passed ten repeated rounds |
| Gateway/resource all-target compile | passed | implementation and fresh code-review verification |
| Independent code review | passed | fresh reviewer distinct from plan reviewer, analyst, and implementer; no technical findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | manifest unchanged; no existing integration test edited |

## Handoff
- Ledger/changelog updated: yes; three changed anchor files
- Commit: pending
