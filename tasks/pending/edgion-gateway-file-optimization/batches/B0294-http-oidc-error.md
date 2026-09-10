# B0294 — HTTP OIDC Error Model

- Pipeline state: analysis-ready; defer candidate to existing D0058, no mutation authorized.
- Active anchor: `edgion-gateway`; baseline `8df2871c`.
- Target: `edgion-gateway/src/plugins/http/openid_connect/error.rs`.
- Analyst `b0294_oidc_error_analysis` examined the full target and all callback/request/token/discovery/JWKS/session response and logging consumers.
- Confirmed high-value security defect: stringly `Other` messages can reach public 401/502 bodies, while discovery/JWKS dynamic Display text can reach PluginLog with provider URL/address/transport detail. A target-only `user_message` patch would leave log leakage and can collapse auth policy distinctions.
- Correct repair is the existing complex D0058 task across the typed error model and every producer/consumer. Do not duplicate it or convert OIDC's dynamic self-written auth responses to `ErrResponse`; canonical gRPC/session behavior must remain.
- Proposed terminal outcome after independent review: deferred to `deferred/D0058-openid-connect-dependency-error-redaction.md` and `tasks/todo/redact-openid-connect-dependency-error-responses.md`. No new task or integration edit.
