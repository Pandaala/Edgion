---
name: oidc-prepared-signing-policy
description: Review the generation-owned OIDC signing policy, delayed algorithm failures, and verifier ordering.
knowledge_type: regression-guards
---

# OIDC signing policy belongs to the plugin generation

## Conclusion and scope

`OpenidConnect::try_new` prepares the expected and allowed signing algorithms
once through `jwt_common::resolve_algorithm_policy`. The private immutable
`signing_policy` owns either the prepared algorithms or a data-free Copy
`AuthFailure::InvalidAlgorithm`. JWT verification borrows this outcome after header parsing (reusing Auto
classification when available), checks the token algorithm, then accesses the provider.

Preparation must not turn a runtime algorithm rejection into constructor
failure. In particular, resource validation accepts `tokenSigningAlg: HS256`
without an explicit allowlist, while the effective default policy rejects it.
Malformed headers still produce `InvalidFormat` before the stored failure.
Auto mode continues to fall back only for invalid format.

## Rationale and evidence

[Graded design rules, section U](../../../00-design-rules.md#u-gateway-hot-path-performance--bounded-memory)
requires reused request policy to be built during configuration preparation.
ARCH-035 identified repeated parsing and allowed-list allocation in
`token_verify.rs::verify_jwt_token_internal`, including both ID-token and
access-token verification in JWT-mode nonce-enabled callbacks.

The resolver retains the exact RS256/RS384/RS512/ES256/ES384 defaults,
case-sensitive parsing, explicit HS/PS/EdDSA opt-in, empty-list rejection and
expected membership checks. Its narrow error type contains no provider text.
The verifier retains runtime token-header validation, key selection and JWKS
rotation. Old captured plugin generations retain their own policy.

## Alternatives and limits

A static default slice alone would leave custom-policy parsing in the request
path. Preparing the complete policy removes both forms of repeated work without
adding a global cache, synchronization, or a shared JWT abstraction. The
[JwtAuth/OIDC split](../../jwt-auth/intentional-design/jwt-oidc-helper-split-deliberate.md)
remains deliberate.

Provider ordering applies to the internal JWT verifier. Constructor discovery
prewarm and callback discovery/token exchange have their own earlier ordering.
No provider-free construction or callback guarantee follows from this change.
ID-token client-ID audience override, nonce checks and
[error projection](dependency-error-projection.md) keep their existing owners.
The structural removal establishes no measured throughput or latency gain.

## Verification and re-evaluation

Helper tests cover exact defaults, opt-ins, malformed/case-sensitive inputs,
empty lists, expected membership and token mismatch. Plugin tests directly
exercise malformed-header precedence and algorithm rejection with cold provider
caches and zero scripted discovery/JWKS GETs. A signed-token test retains an old
plugin generation across construction of a changed policy and checks client-ID
audience and nonce behavior. Fixtures use the existing network-free scripted
provider seam; they do not establish wire-level DNS or TLS behavior.

Re-evaluate when constructor validation, algorithm configuration, JWT dispatch,
provider ordering or plugin generation ownership changes. Fresh command results
are recorded in the ARCH-035 implementation task; existing JWKS rotation and
response-projection tests remain part of targeted OIDC verification.

## Resolution references

- ARCH-035: generation-owned signing-policy preparation in `plugin.rs`,
  `jwt_common.rs` and `token_verify.rs`; regressions in their external unit tests.

## Per-verification header owners

The initial decoded Header remains available through both the first JWKS
selection and the kid-miss forced refresh. Once a DecodingKey has been built,
the verifier releases that header before awaiting discovery. The copied token
algorithm and independently owned decoding key remain available for Validation
and real decode. No JWKS or discovery snapshot lifetime changes here.

After real decode succeeds, its unused TokenData header releases before moving
claims into the returned Value. Partial movement of claims alone would leave
that header until scope cleanup. Both header owners have ordinary owned fields
with no custom destructor in jsonwebtoken 11.1.0. These earlier release points
preserve raw-token verification, key rotation, issuer/audience/expiration and
nonce/scope behavior. They establish no erasure, reduced allocation count or
measured whole-request peak/throughput improvement.

## Auto classification reuses its decoded header

Auto mode decodes a header only when use_jwks is enabled. A successful parsed
Header moves directly into the private verification core; it is not parsed again
between classification and algorithm/JWKS selection. Other access-token,
allow-expired and ID-token wrappers retain their existing verification parameters
and obtain the same parsed header through decode_jwt_header before entering the
core. No cached token/header or new public verifier interface is introduced.

Malformed headers still select introspection directly. Valid headers with invalid
payload format still reach the existing InvalidFormat fallback; fallback failure
retains the original JWT error with the same bounded introspection diagnostic.
Signature, algorithm, issuer, audience, expiry and scope failures do not authorize
fallback. The real cryptographic decode still consumes the original token, and
the initial header still releases after key selection before discovery.

The Auto routing regression provides a successful cached introspection result
for each input, then verifies that authentication failures stay rejected. It
also covers valid JWT selection, opaque/non-object-payload fallback and disabled
JWKS, with scripted provider hit counts. This is local behavior evidence, not
network or throughput measurement. Re-evaluate if classification, fallback rules,
verification wrapper parameters or parsed-header ownership changes.

## jsonwebtoken 11 backend and unknown keys

The workspace enables only the AWS-LC jsonwebtoken 11.1.0 crypto backend, reusing
an existing dependency. This does not install or change Rustls's Ring provider.
Non-exhaustive algorithms fail closed in jwk_matches_alg; deserializable unknown
JWK key types cannot satisfy a known algorithm, even with a matching kid. A kid
match with an unsuitable key never falls back to another key.

Existing defaults and explicit HS/PS/EdDSA opt-ins remain unchanged. Public fixed
tokens signed and verified with the old Ring backend exercise policy resolution,
JWK selection, real signature verification and altered-signature denial for all
12 supported SDK algorithms. Allow-expired tests retain required exp, nbf and
claim-format checks. Auto fallback remains restricted to InvalidFormat; new SDK
error variants must not be broadly remapped to introspection fallback.
