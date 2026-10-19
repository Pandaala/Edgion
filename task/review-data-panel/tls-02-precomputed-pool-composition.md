# Precomputed backend TLS pool-identity composition

Status: deferred optimization for user decision. No runtime change.
Source baseline: 2c94c1565.

## Evidence

Native upstream peer construction reaches create_tls_peer and
backend_tls_group_key in edgion-gateway/src/backends/peer_builder.rs.
BackendTLSPolicyStore::profile_identity already computes the complete policy
fingerprint during publication. PreparedGatewayIdentity already computes the
complete Gateway Secret-data fingerprint during preparation. Certificates, CA
chains and SAN hooks are prepared and retained through owned native handles;
their peer-owned Arcs are not temporary read-only copies to remove.

With no selected Gateway fallback identity, backend_tls_group_key returns the
first eight bytes of the policy fingerprint as a big-endian u64. With fallback,
it constructs a SHA-256 digest over the exact existing domain, the 32-byte policy
fingerprint, the big-endian u64 length 32, and the 32-byte Gateway fingerprint.
It takes the first eight digest bytes as the group key. The current domain has
30 bytes, so this composition hashes 102 bytes; the policy-dependent prefix
including the length frame has 70 bytes. This is bounded stack hashing, not
request-time certificate parsing or scanning of Secret contents.

Pingora includes group_key in its final peer reuse hash. Gateway identity is
selected live at peer construction while the request may retain an older policy
snapshot. Policy-supplied identity takes precedence and suppresses Gateway lookup;
absence of a TLS policy remains plaintext and bypasses identity lookup entirely.

## Decision and alternatives

Choose whether to retain the small fixed hash or evaluate precomputation under
a new IMPL plan:

- Store a cloneable SHA-256 state after hashing the policy-dependent prefix,
  then clone it and append the selected Gateway fingerprint at peer construction.
  This trades saved hashing for additional resident profile state and state-copy
  cost. Profiles that never use Gateway fallback would also carry that state
  unless ownership is designed more selectively. No concrete layout, assembly
  or performance measurement has been established for this alternative.
- Precompute or memoize complete policy/Gateway combinations. This needs an
  explicit owner, cardinality bound, invalidation and concurrency design. A
  request-time map lookup, lock or insertion can cost more than the current fixed
  digest. Cold precomputation must handle independently changing policy and
  Gateway generations without pinning an obsolete Gateway identity to a request.

Neither alternative is implemented. There is no evidence that this hash is a
material request cost, or that a table lookup/state copy improves integrated peer
construction. No new cache, synchronization or digest algorithm is authorized by
this task. A new implementation plan must freeze its exact file allow-list.

## Acceptance for a selected implementation

- Preserve the exact domain, input order, full 32-byte fingerprints, big-endian
  length frame and first-eight-byte truncation. Compare against an independent
  encoding oracle and final Pingora reuse_hash, not only a helper's own output.
  Do not substitute XOR, concatenate truncated group keys or change hashing.
- Preserve live Gateway identity selection alongside captured policy generations.
  Cover replacement, deletion, invalid material and recovery while an old policy
  remains captured. A missing/failed Gateway entry must not reuse a stale cached
  identity or accidentally share a pool with a different trust/identity profile.
- Preserve policy identity precedence, plaintext/invalid-policy handling,
  ancestor admission and identity-error-before-custom-CA-error ordering. Native
  components and group identity must come from the same selected prepared entry.
- Retain complete Secret-data coverage, including extra data fields, CA-mode and
  CA-content isolation, SAN order/type/value isolation, certificate chain and
  private-key changes. Preserve raw Gateway Secret Arc replacement even when data
  is identical; independent probe freshness must not be optimized away.
- Keep the no-fallback path cheap and unchanged in meaning. Any memoization needs
  bounded retention, cancellation/retirement analysis and no blocking request-time
  initialization. Do not add a second TLS preparation or identity authority.
- Benchmark fixed hashing versus candidate state copies/lookups on supported
  targets, including hardware acceleration, many profiles, hot/cold access,
  mixed fallback usage and rotation. Report resident memory and allocation costs
  alongside CPU/latency. A source-level reduction in digest.update calls is not
  a throughput measurement.
- Retain pool-isolation, identity-precedence and rotation tests; independently
  review any live transport verification. No tracing, extra metrics, secret
  exposure, TLS policy weakening or Gateway rustls support changes are implied.

## Original knowledge and verification limits

Read skills/04-review/tls/regression-guards/backend-tls-pool-isolation.md and
backend-client-identity-tls-selection.md in the same directory, plus
skills/01-architecture/02-gateway/04-tls/02-upstream-tls.md. Their constraints
were checked against current source; historical runs are not fresh performance
evidence.

Fresh unchanged-code verification passed 22 peer-builder, 33 BackendTLSPolicy
and three Gateway backend-TLS-store tests, run serially. Formatting, workspace
check/Clippy and document/diff validation passed. No prototype, digest benchmark,
live handshake or connection-pool integration run occurred. The existing parallel
TLS test issues in verification-03-backend-tls-tests.md remain unresolved.
