---
name: outbound-tls-policy-construction-fails-closed
description: Use when reviewing OIDC or shared outbound TLS construction for fallback behavior after CA, mTLS, SAN, or client-builder failures.
knowledge_type: regression-guards
---

# OIDC Outbound TLS Policy Construction Fails Closed

## Rule

When an OIDC configuration declares custom CA, client mTLS, or SAN policy,
failure to resolve or apply any part of that policy must reject plugin
construction. Falling back to the process default HTTP client silently removes
operator-declared trust or identity controls and is not accepted.

The shared outbound TLS resolver owns the same boundary for every consumer:

- local CA and client-certificate references use only their matching locally
  resolved Secret material;
- absent local references may inherit the configured global reference and its
  resolved material;
- a declared reference without all matching material fails closed;
- every declared CA reference must resolve to a Secret containing `ca.crt`;
- every declared custom CA bundle must independently parse to at least one
  certificate; a valid sibling bundle cannot mask an empty, whitespace-only,
  or non-PEM bundle;
- conflicting global client identities keep the reference but clear the
  material, causing the resolver to fail closed rather than choosing one
  nondeterministically.

The existing `verify=false` M4 behavior remains separate: when skip-verify is
actually effective and no SAN policy raises verification, client identity is
intentionally omitted. Do not reinterpret that accepted limitation as
authorization for construction-time fallback.

## Actual owner and captured inheritance

OIDC construction accepts the actual `BoundPluginEntry`, validates its existing
local TLS selection and inherited axes through Common, and derives its namespace
from that owner. Exact metadata/provenance, CA cardinality and client presence
are checked before parsing or entry-cache reuse. Missing material namespace is
not an alias for default; omitted owner/reference namespaces retain canonical
defaults. Disabled entries remain inert. Claim-projection errors retain direct
constructor priority, and schema errors retain runtime priority.

The runtime captures one global-default snapshot for preflight and construction.
The ordinary client uses `build_outbound_client_with_tls_selection`; local SAN
construction uses `resolve_validated_outbound_tls` against the same snapshot and
passes the verified rustls configuration to the canonical HTTP factory. Neither
branch reloads ambient defaults. Existing egress, prewarm, crypto and non-TLS
Secret/session fallback behavior remains separate and unchanged.

EntryCache belongs to one immutable PluginGeneration. Valid source replacement
uses a new generation/cache; occupied-cache invalid candidates only test the
preflight boundary. Unit checks of effective CA/client bytes and constructor
success are not live TLS handshake or Gateway API conformance evidence.

## Review checks

1. Search OIDC construction for any fallback to the default shared client
   after a TLS resolver, rustls, SAN verifier, or reqwest builder error.
2. Verify partial CA lists and unresolved client references return errors. For
   every rustls and reqwest adapter, exercise mixed valid/zero-certificate CA
   bundles in both orders; retain multi-valid and multi-certificate bundle
   coverage.
3. Verify global mTLS-only defaults survive aggregation and conflicting
   identities produce a deterministic fail-closed state.
4. Keep OIDC constructor diagnostics and returned errors static and operator-safe;
   do not format TLS dependency errors or endpoint URLs even on the cold path.
   See [dependency error projection](dependency-error-projection.md).
5. Common tests that mutate the process-wide outbound TLS default use the shared
   RAII test guard, restoring the previous value on return or unwind. Gateway
   tests, where that Common cfg(test) guard is inaccessible, use the existing
   exact-child subprocess helper, restore the previous value on normal return,
   and rely on child process teardown on panic.

## Regression evidence

- OIDC construction tests cover unresolved CA, zero-certificate CA, mixed valid
  and empty CA bundles in both ordinary and SAN-constrained construction,
  unresolved mTLS identity, and the SAN-specific invalid-CA path.
- Shared resolver tests cover partial CA resolution and local/global identity
  ownership and inheritance.
- Gateway verified-rustls and Common LinkSys rustls/reqwest tests cover empty
  and non-PEM siblings in both bundle orders. AllEndpointStatus verifies that
  the same construction failure remains mapped to the generic
  `probe configuration unavailable` response.
- Gateway configuration tests cover mTLS-only global defaults and conflicting
  global identities.
- The complete `edgion-common` library suite passes repeatedly under parallel
  test scheduling without global-default leakage.

## Resolution references

- `OIDC-TLS-01` — 2026-08-29.
- `reject-empty-individual-outbound-ca-bundles` — fixed 2026-09-14; each
  declared bundle is now checked independently in all three TLS adapters.
