# B0293 — HTTP OIDC Callback

- Pipeline state: terminal; callback examined-no-change with a necessary canonical supporting security fix.
- Active anchor: `edgion-gateway`; baseline `8df2871c`.
- Analysis target: `edgion-gateway/src/plugins/http/openid_connect/callback.rs`. Exact technical allow-list for the canonical fix: `edgion-gateway/src/plugins/http/openid_connect/logout.rs` and `edgion-gateway/src/plugins/http/openid_connect/tests.rs`; callback itself needs no duplicate check.
- Analyst `b0293_oidc_callback_analysis` fully examined callback/state/nonce/PKCE/cookies/token secrecy, normalization, canonical clients/TLS/egress/caps, redirects, logs and relevant tests.
- Confirmed: initiation signs normalized `session.get_path()` as `original_url`; `/%5Cevil.example/path` becomes `/\evil.example/path`. The current sanitizer accepts any single-leading-slash target, but browsers interpret this backslash form as an authority replacement, allowing a post-login cross-origin redirect despite authentic state.
- Minimal fix: extend the sole canonical `sanitize_redirect_target` in logout.rs to reject any backslash. Test the real normalization output and raw one/two-backslash paths, existing network-path/absolute rejection and a valid relative path. No second callback sanitizer or schema/state redesign.
- Only the exact pure test filter may run. Exclude DNS, wiremock/TcpListener, global secret-store child-process and ignored cap tests. Preserve all other reviewed OIDC security, redaction, cookie, outbound and access-log behavior.
- No matching task exists; no integration addition requested because deterministic normalization/sanitizer coverage closes this boundary.

## Proposed implementation plan

- Implementer `b0293_implementation` returned a plan without edits/checks. Tighten the canonical sanitizer to accept only a single-leading-slash target with no backslash; otherwise use `/`.
- One pure test imports the public path normalizer without global state, asserts the encoded-backslash normalization output is rejected, pins raw one/two-backslash, network-path and absolute rejection, and preserves a normal relative target.
- Exact test only; no network/global OIDC tests. Fresh plan reviewer `b0293_plan_review` assigned. Stop for API/output drift, new state/network or any wider file/behavior requirement.
- Plan reviewer `b0293_plan_review`: approved, no MustFix. Confirmed actual normalization output, WHATWG authority replacement, conservative backslash rejection, public state-free normalization API and exact two-file scope. Original implementer authorized.

## Implementation, review and verification

- Implementer `b0293_implementation` changed only `logout.rs` and `tests.rs`: the canonical sanitizer rejects any raw backslash and a state-free unit test covers the normalized `%5C` exploit, raw one/two-backslash paths, network-path and absolute targets, plus a valid relative target.
- The first exact filter used a stale module path and selected zero tests; the corrected exact filter `plugins::http::openid_connect::plugin::tests::sanitize_redirect_target_rejects_cross_origin_forms` passed 1/1. No network, DNS, listener, child-process or ignored test ran.
- Fresh final reviewer `b0293_final_review`: LGTM, no MustFix, ShouldFix or deferred finding. The full authorization/callback redirect chain and separate configured-logout path were verified.
- Scoped formatting/diff checks and fresh root format, locked workspace check (10.76s), locked Clippy (15.45s, existing warnings, exit 0), agent-doc, SSA, metrics, Pingora, Gateway tracing, English, frozen-integration and diff gates passed.
- Ledger/changelog updated. Commit `309a1c94`; no integration edit or push.
