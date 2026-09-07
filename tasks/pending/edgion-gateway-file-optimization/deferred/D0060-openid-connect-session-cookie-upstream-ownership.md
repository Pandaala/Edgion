# D0060 — Decide OpenID Connect Session-Cookie Upstream Ownership

## Meta
- Status: resolved
- Severity: high
- Discovered in: B0093, OpenID Connect request-flow analysis
- Category: security-credential-ownership
- Review knowledge: [`skills/04-review/oidc/regression-guards/session-cookie-upstream-ownership.md`](../../../../skills/04-review/oidc/regression-guards/session-cookie-upstream-ownership.md)

## Evidence
Successful session-mode authentication left the encrypted `edgion_oidc_session` cookie on the proxied request. The value is a replayable Gateway credential even though its plaintext is encrypted. `hideCredentials` previously removed only the bearer `Authorization` header, and the session branch returned before applying any cookie removal.

## Why deferred
Whether the Gateway-owned session cookie is always internal, is hidden only when `hideCredentials` is enabled, or remains application-visible is an external request-wire and security contract. Choosing one policy locally would silently change upstream behavior. The approved change must define ownership, document the mode interaction, reuse the canonical multi-line Cookie removal helper, and update all affected request paths atomically.

## Resolution
Gateway-owned cookies (`sessionCookieName` / default `edgion_oidc_session`, and `edgion_oidc_state`) are always stripped before proxying through `auth_common::strip_cookie`, on every `GoodNext` path, independent of `hideCredentials`. `hideCredentials` remains bearer-`Authorization` only. Unrelated application cookies are preserved.

Gating removal on `hideCredentials` or leaving the encrypted session cookie application-visible is not accepted.

## Integration-test impact
No new integration issue was required. Shared Cookie helper coverage plus OIDC `MockPluginSession` request-path tests cover the approved wiring and preserve unrelated application cookies.
