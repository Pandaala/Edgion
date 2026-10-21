# Prepare client-certificate serial inputs for connection reuse hashing

Status: deferred for user decision. Inspected Edgion baseline: `75630765e`.
Scope: supported BoringSSL Gateway and its locked Pingora fork; no runtime change.

## Evidence

The Cargo.lock-resolved Pingora revision is
`1aaa45cc70430bc782990bc20ddee3fef36d6b99`. In that revision:

- `pingora-core/src/upstreams/peer.rs`: `HttpPeer::reuse_hash` recomputes
  `peer_hash`; `HttpPeer::hash` includes `client_cert_key` as well as address,
  scheme, proxy, SNI, verification options, group key and selected H2/TLS options.
- `pingora-core/src/utils/tls/boringssl_openssl.rs`: `CertKey::hash` walks the
  certificate chain in order. For each successful `get_serial`, it hashes the
  returned String; failed conversions are skipped.
- `get_serial` converts the certificate serial to a BigNum, converts it to a
  native hexadecimal string, then copies that string into a Rust String.
  Locked boring 5.2.0 calls `ASN1_INTEGER_to_BN` with a null destination and
  `BN_bn2hex`; this is repeated conversion and temporary ownership, despite
  the surrounding CertKey being held behind an Arc.
- Connector H1 lookup/release and H2 lookup/release reach `reuse_hash`. H2 methods
  already reuse a local computed key within their pool operations. Invocation
  counts depend on protocol, acquisition/release and failure paths; this is not
  a fixed number of conversions per request or proof of a dominant cost.

Edgion prepares native identities during publication, then installs their Arcs
in peers through `backends/peer_builder.rs::create_tls_peer`. Preparing certificates
once does not prepare these serial hash inputs. This differs from
[tls-02](tls-02-precomputed-pool-composition.md), which concerns the small fixed
SHA-256 composition of already prepared policy/Gateway fingerprints.

## Decision and alternatives

Measure mTLS reuse-hash and full connection acquisition/release costs before
selecting a fork change. Compare short and longer chains, H1/H2, cold connections,
reused connections and identity rotation, including retained memory and CPU.
No benchmark or numerical benefit is established here.

One candidate prepares the exact successful serial strings on the CertKey owner
and borrows them during hashing. First establish certificate immutability across
all constructors, returned native references and aliases. Private Rust fields
alone do not prove native object immutability. Preserve serial spelling, signs,
leading-zero behavior, chain order and String Hash framing; substituting raw
serial bytes, a digest, Arc pointer identity or a different hash sequence is not
an equivalent change.

Failure timing is a separate decision: today's fallible conversion is attempted
on every hash. Permanently caching a failed conversion, rejecting construction,
or moving failures to publication is not automatically equivalent. Specify and
review that policy rather than silently converting an intermittent omission
into a permanent one. Do not add request-time initialization or a blocking cache
without an explicit ownership and failure design.

Caching the entire peer hash is a broader alternative. The peer has mutable
hash inputs: Edgion's upstream authority decision can replace TLS SNI after
construction. A cache needs a proven finalization boundary or complete invalidation
for every hashed field and every caller; constructor-time caching is insufficient.
Retaining the current implementation remains valid pending measurements.

## Acceptance and boundaries

Keep Edgion's complete policy/Secret fingerprints and group-key isolation intact.
Certificate serials do not identify full trust, chain or private-key material;
this optimization must not replace the current isolation authority. Preserve
live Gateway identity selection, policy precedence and captured old generations.

Compare legacy and candidate Hash input sequences and final peer reuse hashes
for representative serials/chains and failure cases. Exercise same-serial changed
certificates, changed keys, CA/SAN changes, identity replacement/removal, H1/H2
reuse, and post-construction SNI/options changes. Existing Edgion pool-isolation
tests provide relevant regression coverage but do not establish conversion
allocation counts, alias safety or a new cache's correctness.

Any implementation requires a separately reviewed IMPL plan, explicit fork-file
allow-list, dependency revision policy and actual fork tests. Do not edit the
Cargo checkout as a deliverable or add a second Edgion TLS representation merely
to bypass the dependency API. No rustls support work is included.

Original authority: `skills/04-review/tls/regression-guards/backend-tls-pool-isolation.md`
and `skills/01-architecture/02-gateway/04-tls/02-upstream-tls.md`.
This documentation batch passed formatting, workspace check/Clippy and agent-doc
validation. Existing unrelated warnings remain. No source tests, live handshake
or benchmark ran; these checks do not verify an unimplemented optimization.
