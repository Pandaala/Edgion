---
id: ARCH-035
status: candidate
kind: architecture-debt
priority: P3
partition: PART-331
analyst: /root/analyze_outbound
independent_verification: VERIFICATION-ARCH-035.json
---

# Prepare the OIDC signing algorithm policy per plugin generation

## Current ownership and evidence

`openid_connect/token_verify.rs::verify_jwt_token_internal` (85-88) calls
`jwt_common.rs::resolve_algorithm_policy` for every JWT. The helper reparses
immutable `tokenSigningAlg` and each configured `allowedSigningAlgs` string,
then allocates an allowed-algorithm Vec. Even the default path allocates its
five-algorithm vector. This occurs before provider access and on warm JWKS hits.

Bearer/browser authentication, `handle_callback` nonce validation and refresh
wait-timeout verification all use the same internal verifier. When a
nonce-enabled callback also verifies its access token as a JWT, it rebuilds
the same policy for the ID-token and access-token checks.
`OpenidConnect::try_new` (plugin.rs 273-445) already
owns the frozen generation, prepared secrets, normalized paths, claim plan and
outbound transport. The signing policy lacks that cold owner. Design rules
section U explicitly requires reusable request policy to build at configuration
resolution. This establishes repeated parsing/allocation, not a measured latency
or throughput effect; cryptographic/provider work can dominate.

## Bounded improvement and alternative

Give the existing plugin generation a private prepared signing policy. Reuse the
current resolver during construction; borrow its expected algorithm and allowed
slice during verification. Runtime token header checks, JWK selection, key
rotation and signature verification remain request-dependent. No new factory,
shared JWT framework, global cache, synchronization or dynamic key cache is needed.
A static default slice saves only the default allocation and leaves custom-policy
rebuilds; preparing the small entire policy gives one lifecycle owner.

## Required invariants and risks

Preserve the exact RS256/RS384/RS512/ES256/ES384 defaults, case-sensitive parsing,
explicit HS/PS/EdDSA opt-in, empty-list rejection, expected-algorithm membership
and token mismatch rules. Keep `decode_header` before the prepared policy outcome,
then `validate_token_alg`, all before provider fetch. Old requests retain their
captured generation when configuration changes.

Resource validation currently accepts a recognized `tokenSigningAlg` without an
explicit allowed list even when it lies outside the effective defaults, such as
HS256. Construction can succeed while JWT verification returns the data-free
`Auth(InvalidAlgorithm)`. Simply failing `try_new` would change the invalid-entry
response. Retain that typed failure in the private prepared owner unless a
separate admission change is justified. Do not cache a schema-level
`validation_error: Option<String>`.

Keep the original JWT/OIDC helper split, Auto's invalid-format-only fallback,
callback 401 versus bearer scope 403, session `unauthAction` fall-through,
nonce/PKCE defaults, TinyUFO positive-introspection semantics, rotating JWKS,
claim-header skip/last-write rules and capped exchanges. This issue does not
consolidate the static issuer-scoped `JwtAuth` verifier or duplicate ARCH-031's
orphan reader retirement.

## Evidence and future verification

Original pages read: `jwt-auth/intentional-design/jwt-oidc-helper-split-deliberate.md`,
`oidc/regression-guards/dependency-error-projection.md`,
`oidc/intentional-design/oidc-send-error-extract-function.md`, and the OIDC signing
configuration and current JWT helper, plugin-generation and dependency-error
actual test bodies. Source hashes and precise supporting scopes are recorded in
REPORT-PART-330.json and REPORT-PART-331.json. All 15 OIDC primary files were read
in full by the analyst; current hashes match the frozen inventory.

Future verification should cover default/explicit policies, malformed/empty and
outside-default prepared failures, rejection before provider access, ID-token
audience override and generation isolation using synthetic/scripted fixtures.
No tests, builds, benchmarks, network calls or source changes were executed.
Independent verification is pending.
