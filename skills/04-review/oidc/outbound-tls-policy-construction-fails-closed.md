---
name: outbound-tls-policy-construction-fails-closed
description: Use when reviewing OIDC or shared outbound TLS construction for fallback behavior after CA, mTLS, SAN, or client-builder failures.
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
- a custom CA bundle that parses to zero certificates is invalid;
- conflicting global client identities keep the reference but clear the
  material, causing the resolver to fail closed rather than choosing one
  nondeterministically.

The existing `verify=false` M4 behavior remains separate: when skip-verify is
actually effective and no SAN policy raises verification, client identity is
intentionally omitted. Do not reinterpret that accepted limitation as
authorization for construction-time fallback.

## Review checks

1. Search OIDC construction for any fallback to the default shared client
   after a TLS resolver, rustls, SAN verifier, or reqwest builder error.
2. Verify partial CA lists and unresolved client references return errors.
3. Verify global mTLS-only defaults survive aggregation and conflicting
   identities produce a deterministic fail-closed state.
4. Keep error detail in cold-path diagnostics; plugin construction returns a
   stable operator-safe error.
5. Tests that mutate the process-wide outbound TLS default must use the shared
   RAII test guard, which installs a clean default and restores the previous
   value on normal return or unwind.

## Regression evidence

- OIDC construction tests cover unresolved CA, zero-certificate CA, unresolved
  mTLS identity, and the SAN-specific invalid-CA path.
- Shared resolver tests cover partial CA resolution and local/global identity
  ownership and inheritance.
- Gateway configuration tests cover mTLS-only global defaults and conflicting
  global identities.
- The complete `edgion-common` library suite passes repeatedly under parallel
  test scheduling without global-default leakage.
