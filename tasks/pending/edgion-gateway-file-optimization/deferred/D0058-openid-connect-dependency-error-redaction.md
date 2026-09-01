# D0058 — Redact OpenID Connect Dependency Error Responses

## Meta
- Status: needs-implementation
- Severity: high
- Discovered in: B0091, OpenID Connect final review
- Category: security-error-modeling
- Canonical task: [`tasks/todo/redact-openid-connect-dependency-error-responses.md`](../../../todo/redact-openid-connect-dependency-error-responses.md)

## Evidence
Several OpenID Connect discovery, JWKS, token, refresh, introspection, and userinfo failure paths retain endpoint status, response parsing, or dependency error text in `OidcVerifyError::user_message()` and write it into a fail-close response body. B0091 removed Secret-resolution detail from credential failures but intentionally did not redesign the wider dependency error model.

## Why deferred
Correcting only one token-exchange branch would leave inconsistent public responses and could change the 401/502 and `unauthAction` routing contract differently across endpoints. The fix needs one inventory and typed mapping for all OIDC dependency clients, with static public bodies and useful bounded PluginLog/access-log categories.

## Integration-test impact
No new integration issue is required initially. Typed error, response-writer, and endpoint fixture tests can prove fixed public bodies and retained internal classifications without changing existing frozen integration tests. Reassess only if the approved design changes an externally documented status or challenge contract.
