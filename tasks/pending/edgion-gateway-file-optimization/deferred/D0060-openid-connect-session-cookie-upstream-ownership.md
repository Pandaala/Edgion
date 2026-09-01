# D0060 — Decide OpenID Connect Session-Cookie Upstream Ownership

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0093, OpenID Connect request-flow analysis
- Category: security-credential-ownership
- Canonical task: [`tasks/todo/openid-connect-session-cookie-upstream-ownership.md`](../../../todo/openid-connect-session-cookie-upstream-ownership.md)

## Evidence
Successful session-mode authentication leaves the encrypted `edgion_oidc_session` cookie on the proxied request. The value is a replayable Gateway credential even though its plaintext is encrypted. `hideCredentials` currently removes only the bearer `Authorization` header, and the session branch returns before applying any cookie removal.

## Why deferred
Whether the Gateway-owned session cookie is always internal, is hidden only when `hideCredentials` is enabled, or remains application-visible is an external request-wire and security contract. Choosing one policy locally would silently change upstream behavior. The approved change must define ownership, document the mode interaction, reuse the canonical multi-line Cookie removal helper, and update all affected request paths atomically.

## Integration-test impact
No new integration issue is required initially. The shared Cookie helper already has multi-line coverage, and an OIDC `MockPluginSession` request-path test can prove the approved wiring and preserve unrelated application cookies. Reassess only if the selected contract introduces a cross-plugin or real-proxy interaction that cannot be verified locally.
