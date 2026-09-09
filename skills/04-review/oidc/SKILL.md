---
name: review-oidc
description: Review knowledge for oidc, grouped by decision type.
---

# oidc review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [OpenidConnect `useNonce` / `usePkce` Default Off: Design Tradeoff + Doc Recommendation](intentional-design/nonce-pkce-defaults.md)
- [OIDC `send_oidc_error` leaf extraction — FIXED](intentional-design/oidc-send-error-extract-function.md)
- [OIDC `requiredScopes` Only Enforced When `bearerOnly=true` — ACCEPTED-DESIGN (doc gap fixed, guard kept)](intentional-design/required-scopes-bearer-only-coupling.md)
- [OIDC `resolve_session_secret` / `resolve_client_secret` `Some("")` Fallthrough — NOT-A-BUG (intentional defensive pattern)](intentional-design/resolved-secret-empty-fallthrough.md)
- [OIDC `resolve_session_secret` / `resolve_client_secret` "per-request RwLock+clone" — NOT-A-BUG (premise does not match real behavior)](intentional-design/session-secret-resolve-per-request.md)
- [cookie-auth-05 — OIDC state cookie shares `session_cookie_secure`](intentional-design/state-cookie-shared-secure-flag.md)

## Regression guards

- [OIDC authentication response byte caps](regression-guards/authentication-response-byte-caps.md)

- [OpenID Connect Dial-time Egress Policy — FIXED](regression-guards/dial-time-egress-policy.md)
- [OpenID Connect Discovery Failure Backoff — FIXED](regression-guards/discovery-failure-backoff.md)
- [OIDC Outbound TLS Policy Construction Fails Closed](regression-guards/outbound-tls-policy-construction-fails-closed.md)
- [OIDC Path Redirect URIs Ignore Untrusted `X-Forwarded-Host` — FIXED](regression-guards/redirect-uri-ignores-untrusted-forwarded-host.md)
- [OIDC Path Redirect URIs Derive the Scheme From the Connection — FIXED](regression-guards/redirect-uri-scheme-from-connection-not-forwarded-proto.md)
- [OpenID Connect session/state cookies are Gateway-owned — FIXED](regression-guards/session-cookie-upstream-ownership.md)
