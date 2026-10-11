---
id: ARCH-036
status: candidate
kind: behavior-defect
priority: P2
partition: PART-322
analyst: /root
independent_verification: VERIFICATION-ARCH-036.json
---

# Use the authentication failure contract when HMAC has no usable credentials

## Concrete trigger and current behavior

`hmac_auth/plugin.rs::HmacVerifier::verify_headers_with` checks the available
credential map before parsing a present Authorization header. An absent or empty
map produces `NoCredentials`. `HmacAuth::run_request` handles that outcome by
writing HTTP 500 with `Internal gateway error` and an HMAC challenge, after the
shared auth delay. This differs from ordinary invalid-credential 401 denial.

A nonempty configured secret group remains a valid credential source through
`HmacMatchConfig::has_credentials_source` and `HmacAuthConfig::detect_validation_error`
even when its resolved map is absent. Controller `resolve_credential/hmac.rs`
clears the prior map, skips unusable Secrets/entries, and publishes None if no
credentials survive. For example, a referenced existing Secret missing the fixed
`credentials.yaml` field leaves a structurally valid HMAC configuration with no
usable credentials. Runtime creation still constructs the dedicated HmacAuth;
its generic body gate is deliberately bypassed until cheap authentication passes.

The actual unit test `no_credentials_uses_generic_exact_internal_response`
explicitly asserts status 500, body `500 Internal gateway error`, challenge and
`err: no creds configured`. This is positive evidence of current behavior, not
proof that it meets the requirement. No live incident or executed regression is
claimed.

## Authority and bounded correction

Design rules section F explicitly requires misconfigured/unresolvable auth
Secrets to reply with the plugin's generic 401, never 500; the sole stated
exception is wholly unconfigured Admin bearer auth. The JWT invalid-credential
and JWE resolved-credential original records follow that rule. HMAC is another
auth plugin, not that exception. The generic body is already opaque, but the
status still distinguishes server credential absence from client authentication
failure. The HMAC group-log record permits its private diagnostic and does not
approve a public 500.

Change only this reachable NoCredentials response to the generic HMAC
credential-denial 401 contract. Keep `push_err` attribution, shared delay,
configured realm, no credential contents or provider detail, and the existing
single dynamic response/shutdown sequence. Update the incorrect exact-response
unit test rather than treating its assertion as policy authority. Reuse the
existing shared auth response helper; no new failure vocabulary or auth factory
is needed. Removing the plugin or retaining an older credential generation would
fail open or weaken revocation and is not an alternative.

## Scope, invariants and future verification

Missing Authorization may still follow the existing configured anonymous policy.
A present token with no usable map must remain fail closed. Keep malformed
signature/date/header checks, signed-body digest, no body IO before cheap auth,
physical/logical cap 413 and mechanical body-read 408/500 mapping, generic digest
failure, header hiding, successful group attribution and first-wins resolution.
This does not rewrite generic invalid-configuration sealing or shared HMAC
condition boolean outcomes. An impossible disappeared credential after successful
immutable verification is a separate internal invariant branch.

Future tests should use a structurally valid Secret-group config with None and
empty resolved maps through the actual compile/runtime path, assert exact generic
401/challenge/body/diagnostic and no body capture or forwarding, then cover
missing-token anonymous behavior and valid replacement generations. A resolver
fixture for a present unusable Secret distinguishes credential failure from
operator-shape rejection. Existing network-free mocks/resolver fixtures suffice.

Original rules, HMAC first-wins/group-log/body-order/wire-format/YAML records,
JWT challenge classification and JWE credential-failure policy were searched/read
against current code. Precise hashes and supporting read scopes are recorded in
REPORT-PART-322.json. ARCH-006 already owns cold HMAC size-policy resolution;
this status violation is independent. Static review only; no source/test changes,
builds, test execution, network or Git mutation. Independent verification pending.
