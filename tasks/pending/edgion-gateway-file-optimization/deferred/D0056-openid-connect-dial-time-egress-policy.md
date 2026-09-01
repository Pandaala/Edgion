# D0056 — Enforce OpenID Connect Dial-time Egress Policy

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0090, OpenID Connect audit
- Category: egress-security
- Canonical task: [`tasks/todo/openid-connect-dial-time-egress-policy.md`](../../../todo/openid-connect-dial-time-egress-policy.md)

## Evidence
The default OpenID Connect outbound-client configuration does not install a Gateway `EgressResolver` and does not disable proxy-environment pickup. The SAN-pinning branch builds a separate raw reqwest client without the resolver or `.no_proxy()`. These clients reach the configured discovery URL and remotely discovered JWKS, token, introspection, userinfo, and revocation endpoints, so DNS and proxy routing can bypass dial-time destination classification even though redirects are denied.

## Why deferred
OpenID Connect deployments may intentionally use an internal identity provider, so the correct default and schema cannot be guessed in a JWK ownership batch. The fix must define one policy for configured and discovery-derived endpoints, inject the same resolver into both default and SAN-pinning client construction, disable proxy-environment influence, preserve hostname TLS/SAN verification, and keep timeout, pooling, redirect, cache, and failure semantics aligned.

## Integration-test impact
D0057 tracks the required new end-to-end coverage after the client and policy design is implemented. Existing frozen OpenID Connect integration files remain unchanged.
