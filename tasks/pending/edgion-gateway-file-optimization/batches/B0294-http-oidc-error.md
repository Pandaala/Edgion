# B0294 — HTTP OIDC Error Model

- Pipeline state: terminal; deferred to existing D0058 after independent review and adjudication, with no technical mutation.
- Active anchor: `edgion-gateway`; analysis baseline `8df2871c`, promoted at `309a1c94` after B0293 changed only the separate redirect sanitizer and its unit test.
- Target: `edgion-gateway/src/plugins/http/openid_connect/error.rs`.
- Analyst `b0294_oidc_error_analysis` examined the full target and all callback/request/token/discovery/JWKS/session response and logging consumers.
- Confirmed high-value security defect: `OidcVerifyError::Other` messages can reach public 401/502 bodies. Separately, the no-token `UnauthAction::Auth` discovery path stringifies `JwksProviderError` into `PluginLog`, potentially retaining provider URL, address, or transport detail. JWKS fetch errors are currently wrapped in `OidcVerifyError::JwksFetch` and projected only as static `jwks fetch fail` / `Bad Gateway`; their dynamic strings remain internal.
- Correct repair is the existing complex D0058 task across the typed error model and every producer/consumer. Do not duplicate it or convert OIDC's dynamic self-written auth responses to `ErrResponse`; canonical gRPC/session behavior must remain.
- Proposed terminal outcome after independent review: deferred to `deferred/D0058-openid-connect-dependency-error-redaction.md` and `tasks/todo/redact-openid-connect-dependency-error-responses.md`. No new task or integration edit.

## Independent review and adjudication

- Reviewer `b0294_defer_review` independently confirmed the current-HEAD defect, cross-file scope, D0058 ownership, dynamic self-written auth response semantics and safe continuation. It required narrowing the JWKS logging statement.
- Adjudicator `b0294_adjudication` confirmed the reviewer correction: discovery detail can reach PluginLog and `Other` detail can reach public bodies, while current JWKS projections are static. With the wording corrected, verdict is defer / LGTM.
- No technical or integration-test file changed. Root format, locked workspace check, locked Clippy (existing warnings, exit 0), agent-doc, SSA, metrics, Pingora, Gateway tracing, English, frozen-integration and diff gates passed. Ledger/changelog updated; commit `20e9dc45`, no push.
