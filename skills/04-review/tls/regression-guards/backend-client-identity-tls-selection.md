---
name: backend-client-identity-tls-selection
description: Use when reviewing Gateway backend client identity, plaintext selection, or BackendTLSPolicy identity precedence.
knowledge_type: regression-guards
---

# Backend client identity does not select TLS

## Conclusion and scope

For ordinary Gateway API backend peer construction on the supported BoringSSL
Gateway, `Gateway.spec.tls.backend.clientCertificateRef` supplies credentials
only. An absent BackendTLSPolicy produces a plaintext peer without consulting
Gateway identity material. An effective policy selects TLS; an invalid policy
fails closed rather than becoming plaintext.

After TLS is selected, `BackendTLSPolicy.spec.resolved_client_certificate`
takes precedence over the Gateway fallback. When policy identity exists, the
Gateway identity store is not queried. Without either identity, the effective
policy still selects TLS, without a client certificate.

## Rationale and evidence

- The vendored Gateway API contract in
  `config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml`, Gateway
  `spec.tls.backend`, explicitly separates client details from backend TLS
  selection. See also the
  [upstream TLS architecture](../../../01-architecture/02-gateway/04-tls/02-upstream-tls.md).
- `edgion-resources/src/resources/gateway.rs`: `GatewayBackendTLS` separates
  the operator's `client_certificate_ref` from Controller-resolved
  `resolved_client_certificate`. The latter is schema-hidden and redacted
  for admin serialization while remaining available for conf_sync.
- The Controller Gateway handler resolves the Secret; Gateway
  `runtime/handler.rs::update_gateway_backend_tls_store` publishes only actual-field-paired material under
  the unchanged raw Gateway key, or removes the slot when declaration/material is absent or invalid.
- `backends/peer_builder.rs::peer_from_backend_tls_lookup` matches
  `BackendTlsPolicyLookup::{Absent, Effective, Invalid}`. Only `Effective`
  calls `create_tls_peer`, whose argument requires an effective policy.
  The plaintext branch returns before any identity lookup or parsing.
- `create_tls_peer` selects policy identity before querying the Gateway store.
  Backend consumers in `backends/mod.rs` and the explicit HTTP peer path in
  `routes/http/proxy_http/pg_upstream_peer.rs` use the same lookup-based builder.
  `extract_tls_config` reports plaintext for `Invalid`, but peer construction
  rejects `Invalid` before any connection is attempted; this helper result
  is not permission to downgrade a failed policy.

## Alternatives and limits

GTLS-01 originally described an identity-only Gateway causing an unintended
mTLS connection to a plaintext backend. That was a real correctness defect,
already corrected before this closure audit. The original task described a
no-policy return inside `create_tls_peer`; the current implementation moved
that decision into `peer_from_backend_tls_lookup`. The older function location
is historical, not the current contract.

This rule does not remove Gateway reference/status validation or Secret
resolution. It does not redefine TLS selection for separate explicit outbound
URL mechanisms. Connection reuse after trust or identity changes is covered by
the separate [pool isolation guard](backend-tls-pool-isolation.md).
Gateway rustls remains outside the supported product boundary; see the
[rustls decision](../intentional-design/rustls-gateway-backend-unsupported.md).

## Verification and re-evaluation

Existing `peer_builder` tests cover all identity-source presence combinations,
an absent policy with deliberately unusable Gateway Secret material (plaintext,
no client certificate, no parsing error), and invalid-policy rejection.
`Gateway/TLS/BackendTLSGatewayCert` contains two real-network cases: policy-selected
mTLS to port 30052 and identity-only plaintext to port 30001, through the same
Gateway. These are static scenarios, not live Secret-rotation coverage.

Fresh verification on 2026-09-08:

- `cargo test -p edgion-gateway --lib backends::peer_builder::tests`: all
  12 tests passed, including the identity-only and invalid-policy regressions.
- `./edgion-tests/integration/scripts/integration/run_integration.sh -r Gateway
  -i TLS/BackendTLSGatewayCert`: both network cases passed after the canonical
  build preparation. `tmp/integration_testing/testing_20260908_171704/report.log`
  records PASS, 18 report entries passed, no failures or skips, including final
  metrics-label, unknown-field, requeue-loop, and cache-residue gates.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` completed successfully. Clippy emitted
  existing warnings in unchanged code; this closure changes no Rust files.
- Agent-doc validation, SSA-force, metrics-inventory, and Pingora-source-policy
  scripts initially passed. A final agent-doc rerun found an unrelated broken
  ACME knowledge link to the cleanup-response task (now maintained in the
  [ACME cleanup response guard](../../acme/superseded/dns01-retired.md));
  closure was deferred pending resolution of that missing target and a passing rerun.
  No full-workspace test run was requested or performed.
- Independent read-only review returned LGTM for the current source verdict,
  knowledge classification, scope, and evidence boundaries.

Follow-up closure verification on 2026-09-09:

- Current source still selects plaintext before Gateway identity lookup for
  an absent policy, rejects invalid policies, and prefers policy identity.
- `cargo test -p edgion-gateway --lib backends::peer_builder:: --locked`
  passed all 15 tests, including the connection-pool isolation regressions.
- Agent-doc validation passed; the unrelated documentation blocker above
  no longer prevents closure. SSA-force, metrics-inventory, and
  Pingora-source-policy guards also passed.
- `cargo fmt --all`, workspace all-target `cargo check`, and workspace
  all-target `cargo clippy` passed. Clippy reported warnings in unchanged
  code. Independent source and knowledge review returned LGTM.
- The live integration results above remain historical; this follow-up did
  not rerun the integration topology or the full-workspace test suite.

The source task's 2026-08-30 test/check record remains historical evidence,
separate from the fresh results above. This audit changes no runtime behavior,
configuration, public API, or user documentation.

Re-evaluate when policy lookup states, peer constructors, Gateway identity
publication, identity precedence, or supported TLS backends change.

## Resolution references

- `GTLS-01` — `gateway-backend-client-identity-must-not-enable-tls`, original
  P1 correctness defect, task created 2026-08-29 and implementation recorded
  complete 2026-08-30. Audit on 2026-09-08 confirms the defect was already
  fixed, with fresh regression verification and no production change. Final
  closure verification on 2026-09-09 confirms the unrelated agent-doc blocker
  is resolved.

## Actual Gateway backend client material publication

GatewayBackendTlsStore::update_from_gateway accepts the actual Gateway and its
transport slot, never a detached Secret writer. GatewayTlsRefs selects the native
backend client field; its material accessor shares BoundTlsMaterial identity and
provenance checks before cold publication. Unsupported native group/kind, invalid
owner, mismatched name/namespace, absent declaration or missing material withdraws
the existing slot. Omitted material namespace is default only for this finite
Gateway backend field, matching Controller acquisition; explicit empty namespace
is invalid. Raw publication keys retain their existing key_name form.

Both ordinary peers and AllEndpointStatus probes read the resulting Arc<Secret>.
No request-path owner lookup or new Grant policy is introduced. Withdrawal leaves
the existing fallback behavior: BTP still selects TLS and its own identity takes
precedence; without either identity it uses TLS without a client certificate.
Pairing does not replace independent cryptographic validation. Existing probe
Arc freshness rejects a replaced/withdrawn generation while its captured bytes
remain immutable. Focused owner/store/peer/probe and full/partial handler tests
cover these boundaries; historical network results above are not a fresh live
conformance or handshake claim for this change.
