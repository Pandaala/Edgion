# D0154 — OpenID Connect JWKS Unit-test Network Isolation

- Status: pending after B0298 independent review and adjudication.
- Canonical standalone task: [isolate-openid-connect-jwks-unit-tests-from-network.md](../../../todo/isolate-openid-connect-jwks-unit-tests-from-network.md).
- Twenty-four default-run unit tests across OIDC `tests.rs` and `jwks_provider.rs` start loopback listeners or attempt a loopback connection. Their cache, provider, status, stale, singleflight and Dye coverage is valuable, so isolation requires a focused testability refactor rather than deletion or `ignored` markers.
- Existing `edgion-tests/**` remains frozen and unchanged.
