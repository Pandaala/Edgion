---
name: gateway-admin-bearer-boundary
description: Use when reviewing Gateway Admin API authentication responses, router layering, token handling, or probe separation; the configured-token boundary challenges 401 responses with a fixed Bearer realm, while an unconfigured server fails closed with a non-challenging retryable 503.
knowledge_type: regression-guards
---

# Gateway Admin Bearer Authentication Boundary

## Conclusion

The Gateway Admin API is protected by one shared bearer-token middleware on the
fully merged admin router. Its response contract is:

- a correct token proceeds to the selected handler;
- a missing, malformed, or incorrect token returns `401 Unauthorized`, the
  exact `WWW-Authenticate: Bearer realm="edgion-gateway-admin"` challenge, and
  a generic JSON error body;
- no configured server token returns `503 Service Unavailable` with
  `Retry-After: 10` and no authentication challenge.

The 503 branch describes server readiness, not rejected client credentials. It
must not be collapsed into the 401 branch or advertise a challenge that cannot
succeed.

RFC 6750 section 3 requires a bearer-protected resource server to include a
`WWW-Authenticate` challenge when credentials are rejected. The fixed realm
identifies this Admin protection space without reflecting credentials or
adding `error` / `error_description` details that would distinguish rejection
classes. Missing, malformed, and incorrect credentials therefore receive the
same parameterized challenge and generic body.

## Security Boundary

`AdminAuthState::from_token` trims the startup-resolved token and stores only a
SHA-256 digest. `evaluate` hashes a presented bearer token and compares the two
fixed-size digests with `subtle::ConstantTimeEq`. Neither rejection response,
headers, tracing, nor metrics include the submitted or configured token.

`create_admin_router` merges integration-testing routes before applying the
middleware. Testing routes therefore inherit the same authentication boundary;
they do not need a second middleware. The dedicated probe router contains only
liveness and readiness endpoints and remains separate from the Admin router.

This authentication layer complements rather than replaces the loopback bind
default and mandatory `/configclient` secret redaction.

## Regression Coverage

The real Axum middleware/router tests in
`edgion-gateway/src/api/admin_auth.rs` lock in:

- the exact parameterized Bearer challenge for missing, malformed, and
  incorrect credentials;
- successful handler reachability for the correct token;
- retryable, non-challenging 503 behavior when unconfigured; and
- non-reflection of submitted tokens in rejection bodies.

Pure parsing, trimming, digest, and outcome tests remain as lower-level
coverage.

The process-level integration script
`edgion-tests/integration/scripts/integration/test_gateway_admin_auth.sh`
checks the no-token, missing-token, valid-token, and probe-router outcomes.
Its source-path comment was aligned with the current `admin_auth.rs` location
under D0003; the correction does not change the test behavior.

## Re-evaluation Triggers

- The Gateway Admin API adopts a different authentication scheme.
- Token resolution or middleware placement changes.
- Probe endpoints move onto the Admin router.
- A response starts including credential-specific diagnostic detail.

## References

- `edgion-gateway/src/api/admin_auth.rs`
- `edgion-gateway/src/api/mod.rs::create_admin_router`
- `edgion-gateway/src/api/mod.rs::create_probe_router`
- `skills/04-review/admin-api/regression-guards/gateway-admin-loopback-default.md`
- `skills/04-review/admin-api/mandatory-design/gateway-admin-configclient-redaction.md`
