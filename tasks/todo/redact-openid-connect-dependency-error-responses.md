# Redact OpenID Connect Dependency Error Responses

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | security |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Give every OpenID Connect dependency failure a typed internal classification and a fixed public response so endpoint, status, parsing, TLS, DNS, and transport detail cannot leak through fail-close bodies.

## Requirements

- Inventory discovery, JWKS, authorization support, token exchange, refresh, introspection, userinfo, and revocation errors from client construction through response writing.
- Separate public status/body/challenge from internal PluginLog and access-log classifications; public bodies must be static and detail-free.
- Preserve the auth/non-auth failure-policy shapes, `unauthAction` routing, auth-failure delay, Bearer challenge, Dye outcomes, stale-cache policy, SAN metrics, and best-effort logout/revocation semantics.
- Keep request-level observability in the canonical access log. Do not add tracing or per-request event logs.
- Do not log or return endpoint URLs, resolved addresses, proxy details, provider response bodies, parser text, tokens, cookies, claims, client credentials, or Secret metadata.
- Prefer one typed error-to-wire mapping over endpoint-local string formatting; update all affected producers and consumers atomically.
- Preserve the already implemented dial-time egress boundary documented in
  `skills/04-review/oidc/regression-guards/dial-time-egress-policy.md`: canonical
  resolver, proxy-env disabling, redirect denial and literal-address checks.
  This task changes public error projection, not destination authorization.

## Acceptance criteria

- Every fail-close OIDC dependency response uses the approved fixed status, body, and challenge without internal or provider detail.
- PluginLog/access-log output uses bounded static categories that retain actionable failure ownership without sensitive values or high-cardinality labels.
- Tests cover each dependency class, 401/502 routing, delay/challenge ordering, stale-cache and fail-open boundaries, `unauthAction`, and writer failures.
- Existing integration tests remain unchanged unless an approved public wire-contract change genuinely requires a new standalone scenario.
- Workspace format/check/Clippy and repository guards pass.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Schedule the cross-endpoint error-model redesign separately. | The credential-lifecycle fix could safely remove credential detail leaks, but changing one dependency branch would leave inconsistent OIDC public responses. |
