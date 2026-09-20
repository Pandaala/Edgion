---
name: webhook-auth-hmac-fail-closed-upsert-guard
description: Historical HMAC upsert rejection for the removed WebhookAuth model, not evidence of current authentication enforcement.
knowledge_type: superseded
---

# LinkSys webhook-auth WebhookAuth::Hmac Silent No-op in inject_auth

## Current conclusion

The manager upsert guard no longer exists, and the legacy WebhookAuth enum is no
longer the type of WebhookServiceConfig.auth. Current Webhook requests use
declarative request fields, including Secret-backed
header templates. `WebhookServiceConfig.auth` is still an accepted-but-ignored
JSON placeholder; this predates the unified runtime refactor and does not provide
HMAC signing or fail-closed rejection. Do not use the historical verdict to
suppress findings against current behavior. Removing ignored legacy fields is
separate schema-cleanup debt, not a runtime ownership fix. See the current
[Webhook contract](../../../01-architecture/03-controller-gateway-link/webhook.md#migration-operator-facing).

## Historical resolution (inactive)

**Status / verdict**: fixed (link-sys-F005, 2026-05-25)

When reviewing LinkSys webhook-auth findings of the form "`WebhookAuth::Hmac` is a silent no-op in `inject_auth` / a webhook configured with `auth.type=hmac` registers successfully but sends every outbound call unsigned / no upsert-time guard rejects unimplemented HMAC (link-sys-F005)": **fixed (link-sys-F005, 2026-05-25)**. `WebhookManager::upsert` (`edgion-gateway/src/link_sys/providers/webhook/manager.rs`) now rejects `WebhookAuth::Hmac` with a clear apply-time `Err` (`"auth.hmac is not yet implemented; remove the auth field or use bearer/basic/apiKey(header)"`), mirroring the existing `ApiKey { in: "query" }` guard — fail-closed: the webhook is never inserted into the manager map, so `get_webhook_call_handle` returns `None` and no unsigned outbound call can ever be made. The rejection surfaces in the Gateway log at registration (the `init` task `tracing::error!`-logs the `upsert` `Err`; like the sibling ApiKey-query and TLS-build-failure guards there is no synchronous CRD-status rejection) instead of silent unauthenticated requests. The `inject_auth` HMAC arm (`call.rs`) is now unreachable (its stale "requires body bytes" comment was replaced to say so). **Full HMAC request signing was intentionally NOT implemented** — it needs design decisions (signing header name, canonical string, timestamp, algorithm) that belong in a brainstormed task, and the project's [response-signature-not-needed.md](../intentional-design/response-signature-not-needed.md) + [hmac-auth/](../../hmac-auth/) posture is "don't stack non-industry-standard security layers in the proxy layer". The Low ApiKey-query sub-finding's suggested `call.rs` fallthrough hardening (`tracing::warn!` / `return Err`) is **not accepted**: that arm is already unreachable (guarded at upsert) and `inject_auth` runs on the outbound request path where Iron rule 3 (`skills/03-coding/01-log-safety.md`) forbids `tracing::`; converting it to `return Err` would force the fn to return `Result` for a dead branch. Regression: unit test `test_manager_hmac_auth_returns_error`. Re-reports of the silent-no-op / missing-guard form are not accepted; verify the current `upsert` rejects `WebhookAuth::Hmac` before re-reporting.
