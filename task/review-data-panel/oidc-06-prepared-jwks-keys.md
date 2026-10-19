# Evaluate generation-owned OIDC decoding keys

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `09b0efa7c`.

## Evidence and decision

verify_jwt_token_internal in
`edgion-gateway/src/plugins/http/openid_connect/token_verify.rs` selects a JWK,
then calls DecodingKey::from_jwk on every JWT verification. JwksProvider caches
Arc<JwkSet> snapshots, not decoded key material. Locked jsonwebtoken 9.3.1 decodes
RSA modulus/exponent, EC coordinates, Ed key bytes or the octet secret into owned
buffers. This is repeated decoding/material construction, not evidence that the
entire cryptographic verification algorithm could be precompiled.

Choose whether to retain selected-key preparation, prepare every entry when a
JWKS generation is published, or lazily initialize entries within that generation.
Eager preparation can remove repeated selected-key decoding but adds publication
CPU for unused keys and retains decoded buffers alongside the raw set. Lazy
preparation avoids unused work but adds initialization/synchronization design.
Compare costs across key families, set sizes, request rates and rotations before
choosing. No candidate implementation or benchmark has run.

## Semantic boundaries

A structurally valid, nonempty fetched JwkSet currently publishes even when some
key material cannot be decoded. Only selecting such a key yields Auth(InvalidKey).
Do not turn one malformed unused key into a fetch failure, reject the whole set,
retain an old stale set instead, or skip the bad key in favor of another entry.
A prepared representation must preserve a bounded, data-free per-entry failure
and return it at the existing selected-key boundary.

Selection order is observable. With kid, JwkSet::find chooses the first matching
entry and algorithm incompatibility returns NoSuitableKey; it does not continue
through duplicate identifiers. Without kid, select_jwk chooses the first
algorithm-compatible entry. Only KidNotFound triggers the existing forced refresh.
Keep all algorithm metadata, key-family and curve checks, including explicit
algorithm opt-in. Do not collapse entries into a last-wins map or introduce a
cross-provider/global kid cache.

Publish raw metadata and corresponding prepared results as one immutable
snapshot, retaining captured generations for active requests. Prevent mixed
metadata/key epochs during rotation. Evaluate preparation outside the provider
state write lock; do not hold that lock across expensive work or verification.
Retain the canonical refresh coordination, cancellation ownership, TTL and minimum
refresh timing, discovery failure backoff, stale fallback and exact served_stale
outcomes. Review publication timestamp placement explicitly if preparation moves
into the fetch path. Keep selected-key errors before the verifier's subsequent
discovery lookup, as in the current flow.

Preserve response byte caps, empty-set rejection, dial-time egress/TLS policy,
fixed error projection and existing access-log outcome ownership. Do not add raw
key, octet secret, token or provider details to Debug, logs or metrics. No new
provider service, cache policy, schema knob or crypto backend is authorized here.
JwtAuth's prepared static credentials have a different validation model; the
intentional JwtAuth/OIDC helper split remains in force.

## Acceptance for a later implementation

Use IMPL with an exact allow-list and independent design review. Differential
selection/error tests must include all supported key families, absent kid,
duplicate kid, incompatible algorithms/curves, malformed selected and unselected
material, empty sets and forced-refresh misses. Preserve real signed-token and
nonce/audience tests. Test rotation with old readers, concurrent refresh, stale
responses, throttling, fetch failure without a cache and cancellation. Use the
existing scripted provider seam; unit tests must not dial.

Compare request allocations/CPU against publication CPU, retained raw plus decoded
bytes, concurrent old/new generations, large sets and mostly-unused keys. Include
cold start and rotation frequency; do not benchmark only warm borrowed-key decode.
Keep logical response bounds distinct from heap usage and do not claim that a
JWKS wire-byte cap alone establishes a tight prepared-memory bound.

Fresh unchanged-code verification passed 26 JWKS-provider tests. These establish
a baseline, not candidate equivalence or preparation cost. No benchmark ran.
Original authority: `skills/04-review/oidc/regression-guards/prepared-signing-policy.md`,
`skills/04-review/oidc/regression-guards/discovery-failure-backoff.md`,
`skills/04-review/oidc/regression-guards/dependency-error-projection.md` and
`skills/04-review/jwt-auth/intentional-design/jwt-oidc-helper-split-deliberate.md`.
