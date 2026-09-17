---
name: oidc-jwks-unit-tests-without-network
description: Use when changing OpenID Connect JWKS or discovery unit tests, or the cfg(test) scripted HTTP seam on JwksProvider.
knowledge_type: regression-guards
---

# OpenID Connect JWKS unit tests isolate the outbound GET

## Conclusion and scope

Default-run OpenID Connect JWKS and discovery unit tests exercise the real
cache, TTL, min-refresh, stale-while-revalidate, singleflight, discovery
failure backoff, document validation, HTTP-status mapping, injected
timeout/transport outcomes, key rotation, and Dye mapping. Only the outbound
HTTP GET is replaced by an in-memory `ScriptedHttp` under `cfg(test)`.
Production `JwksProvider` retains the injected canonical `reqwest::Client`,
dial-time egress admission, response byte caps, and data-free
`OidcDependencyError` projection.

The scripted routes, live JWKS atomics, and `install_scripted_http` helper do
not exist in production builds. Default-run JWKS tests install a script so they
never dial. Tests that omit a script, including the ignored local-IdP modules,
still use the injected `reqwest::Client`.

Link-local URLs rejected by `admit_oidc_outbound_url` before GET are outside
this seam. Ignored local-IdP tests in `dependency_error_integration.rs` and
`auth_response_caps_integration.rs` still perform real discovery/JWKS HTTP
when run with `--ignored`; they cover status, parse, and byte caps, not
reqwest timeout or connect classification.

## Rationale and evidence

The canonical testing boundary is [design rules, section X](../../../00-design-rules.md#x-testing-discipline):
unit tests do not hit a network, real database, or real server.

`jwks_provider.rs` previously started a `TcpListener` mock IdP and Wiremock
servers from 22 default-run tests, plus a deliberate connect to
`127.0.0.1:1`. Two default-run Dye tests in `plugin` tests also called
`MockServer::start()`. Those sockets violated the unit-test rule and had
already required `Connection: close` workarounds under parallel load.

The replacement keeps the same 24 contracts. Cache and backoff tests drive
`get_or_fetch_discovery` / `get_jwks` through scripted HTTP 200/5xx bodies.
Timeout and unreachable cases inject `OidcDependencyFailure::Timeout` /
`Transport` rather than sleeping on a real socket. Dye tests install the
same script on the plugin-owned provider and still assert
`jwks-stale-cache` / `jwks-unreachable`.

Sibling: [RequestMirror writer isolation](../../request-mirror/regression-guards/unit-tests-without-network.md).
Discovery backoff semantics remain those in
[discovery failure backoff](discovery-failure-backoff.md).

## Alternatives and limits

Do not bind a loopback server, point at TEST-NET, or `#[ignore]` these tests
to satisfy the no-network rule. Do not add a production HTTP trait, extra
lock, or request-path branch for mock convenience; the seam is `cfg(test)`
only.

The script is not evidence for reqwest wire framing, DNS, TLS, or kernel
connect failures. Timeout and unreachable unit tests inject already-classified
`OidcDependencyFailure::Timeout` / `Transport`; they do not call
`OidcDependencyError::transport` on a live `reqwest::Error`. Ignored local-IdP
tests cover real HTTP status, parse, and byte-cap paths for discovery/JWKS
when run with `--ignored`; they are not timeout or connect-classification
tests. Existing `edgion-tests/**` files were not modified.

## Verification and re-evaluation

Verified on 2026-09-16:

- `cargo test -p edgion-gateway --lib openid_connect::jwks_provider`: 26 passed.
- `cargo test -p edgion-gateway --lib test_get_or_fetch_jwks`: 2 passed.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy retained the existing
  `edgion-tests` `redundant_async_block` warning.
- Agent-doc, SSA-force, metrics-inventory, and Pingora-source-policy guards
  passed. Independent read-only review returned LGTM after the unscripted
  GET fallthrough fix. Existing integration tests were not changed or rerun;
  the full workspace test suite was not run.

Re-evaluate when JWKS/discovery fetch, failure backoff, Dye mapping, or the
unit-test runtime changes. New JWKS unit tests that would otherwise GET must
install `ScriptedHttp` (or seed caches and fail before dial) rather than
start a server.

## Resolution references

- `isolate-openid-connect-jwks-unit-tests-from-network.md`: fixed test-isolation
  defect for the 24 default-run JWKS/discovery unit tests.
