# Reject Empty Individual Outbound CA Bundles

## Meta

- Status: pending
- Type: correctness
- Priority: P2

## Problem

Outbound TLS adapters currently reject a custom trust configuration only when
all declared CA bundles collectively yield zero certificates. A valid bundle
therefore masks another empty, whitespace-only, or non-PEM bundle. The resolver
checks reference and key presence but can carry an empty `ca.crt`, so this state
is reachable from operator configuration.

The affected aggregate-only checks are:

- `edgion-gateway/src/tls/validation/outbound_client.rs` in
  `build_verified_outbound_tls`;
- `edgion-common/src/link_sys_core/tls.rs` in
  `build_rustls_client_config`;
- `edgion-common/src/link_sys_core/tls.rs` in `apply_custom_ca_pem`.

This is a fail-closed construction defect: a partially unusable declared trust
configuration is silently accepted and may become an availability failure during
CA rotation. It is not a trust bypass; only certificates parsed from the valid
subset become trust roots.

## Required Design

Use a plan-first cross-adapter change. Require every declared custom CA bundle to
yield at least one certificate while retaining aggregate bounds and the existing
root-store behavior. Do not patch only one adapter.

Preserve:

- `resolve_outbound_tls` as the canonical policy and secret-resolution owner;
- system roots versus custom-root replacement semantics;
- mTLS identity, SAN verification and `verify` behavior;
- canonical `build_outbound_client`, dial-time `EgressResolver`, disabled proxy
  environment and denied redirects;
- existing generic user-facing failure responses, secret redaction and restrained
  observability.

Update the owning rule in
`skills/04-review/oidc/regression-guards/outbound-tls-policy-construction-fails-closed.md`
so it explicitly covers each declared bundle rather than only the aggregate.

## Verification

- For each rustls/reqwest adapter, prove mixed `[valid, empty]` and
  `[valid, non-PEM]` inputs fail regardless of ordering.
- Retain passing cases for multiple valid bundles and multiple certificates in
  one bundle, plus existing single-invalid fail-close tests.
- Prove ordinary and SAN-constrained OIDC construction reject mixed bundles.
- Prove AllEndpointStatus maps the failure to its existing generic unavailable
  response without exposing certificate or Secret details.
- Run focused Gateway outbound TLS/SAN/OIDC tests, common LinkSys TLS tests and
  the full project pre-commit checks.
- Keep existing integration tests unchanged. If cross-process coverage is later
  required, track it in a separate standalone task.
