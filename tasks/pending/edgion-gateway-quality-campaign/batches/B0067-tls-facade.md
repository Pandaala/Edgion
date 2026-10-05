# B0067 — TLS Route Facade

## Scope

- Baseline: `f28a86045acb66060bb2efe012ee6f7117c4478a`; revalidated at `246a61118f32cc8a4e9a335b07a3dae52cde428f` after concurrent TLSRoute changes.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/tls/mod.rs`.
- Exact implementation allow-list: `edgion-gateway/tests/tls_passthrough_clienthello.rs` only.
- Existing `edgion-tests/**` and protected user paths remain forbidden.

## Context and value gate

- Initial analysis verdict was `examined-no-change`; current-HEAD reanalysis changed the outcome to `change` with simple complexity.
- The facade exposes one coherent TLS route tree whose terminate and passthrough listeners retain distinct transport ownership while sharing canonical routing, plugin, backend, relay, and logging mechanisms.
- Terminate uses the supported BoringSSL endpoint and forwards plaintext TCP after handshake; passthrough uses bounded ClientHello SNI inspection and replays all inspected bytes unchanged.
- External canonical parent/status-owner work correctly made unstamped direct routes non-serving, but missed the standalone socket fixture. Its positive replay control now times out because the fixture publishes no accepted attachment.

## Verified invariants

- SNI, ALPN, certificate callback, exact logical Gateway ownership, route-generation pinning, and fail-close publication ordering remain coherent.
- Both listener modes run only TLSRoute Stage 2; terminate exposes negotiated TLS identity while passthrough does not invent one.
- Connection outcomes remain owned by the canonical TLS access-log context with existing redaction and accepted SNI policy; no hot-path tracing or duplicate mechanism exists.
- Rustls-only gaps are outside the supported Gateway boundary and were not considered.

## Verification

- Current-HEAD TLS module verification passed: 73 tests.
- The current-HEAD socket matrix deterministically failed its positive control with a three-second timeout; reanalysis traced it to the missing `spec.resolvedStatusController` fixture stamp.
- The approved correction stamps the direct fixture with `edgion.io/gateway-controller` before publication while retaining absent status and every transport assertion.

## Deferred findings

- None. The broader startup-cloned `securityProtect` audit is already tracked by D0012.

## Implementation

- Production TLS code remained unchanged after current-HEAD reanalysis confirmed its ownership and generation invariants.
- The standalone passthrough socket fixture now carries the same canonical `resolvedStatusController` stamp that Controller conf_sync assigns before publication; absent status continues to exercise optimistic initial acceptance.
- No production admission rule, transport assertion, log assertion, or frozen integration file changed.

## Independent review

- The final reviewer returned LGTM with no Must Fix, Should Fix, or deferred finding.
- Review confirmed the stamp models Controller publication rather than bypassing acceptance, and every malformed-length, no-dial, replay, reverse-traffic, access-log, and duplicate-log assertion remains intact.

## Final verification

- The socket regression failed deterministically before the correction and passed afterward; all 73 TLS module tests passed.
- Formatting, diff, workspace check/clippy, agent-doc, SSA-force, metrics-inventory, Pingora-source-policy, Gateway API transform contract, no-tracing, cargo-deny, English-only, and frozen-path checks passed.
- Clippy retained only the known unrelated Kafka and frozen integration-test warnings. No full test suite was requested or run.
- Ten existing and nine new frozen integration paths were reconciled only after exact blob/mode attribution to external commits `04413c3e`, `ab459fba`, and `c6aa6e90`.

## Handoff

- Fixture correction committed as `b7af467d`.
- Next campaign anchor: `edgion-gateway/src/routes/tls/passthrough_proxy.rs`.
