---
name: backend-tls-pool-isolation
description: Use when reviewing BackendTLSPolicy trust or client-identity rotation, upstream peer reuse hashes, or additional SAN pool isolation.
knowledge_type: regression-guards
---

# Backend TLS connection reuse follows effective trust and identity

## Conclusion and scope

The supported BoringSSL Gateway path composes the effective BackendTLSPolicy
profile and selected Gateway fallback identity into `HttpPeer.group_key`, even
when no additional SAN is configured. Pingora includes that value in the reuse
hash used by its HTTP/1 idle pool and HTTP/2 in-use and idle pools.

The original `backend-tls-policy-connection-pool-isolation` task described a
SAN-only discriminator. That production defect was already corrected before
this verification work; this change adds regression evidence and updates stale
knowledge, without changing production TLS behavior.

## Rationale and evidence

- `edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs`:
  `profile_identity` computes a domain-separated SHA-256 fingerprint during
  immutable policy publication. Inputs include system/custom trust mode,
  complete `resolved_ca_certificates` CA payloads, `validation.hostname`, typed
  `validation.subject_alt_names`, and all data entries in
  `resolved_client_certificate`, including the full chain and private key.
  CA payloads are sorted and deduplicated; variable-length values are framed.
- `edgion-gateway/src/backends/peer_builder.rs`: `create_tls_peer` selects policy
  identity first, otherwise a Gateway identity, otherwise no identity.
  `backend_tls_group_key` combines the profile fingerprint with all selected
  Gateway Secret data. The same captured Secret is parsed for the peer and
  hashed, so a concurrent store replacement cannot mix those two inputs.
- `peer_from_backend_tls_lookup` and direct `create_tls_peer` consumers in
  `backends/mod.rs` and `routes/http/proxy_http/pg_upstream_peer.rs` share this
  construction. SAN hook installation does not replace the profile key.
- The Cargo.lock-resolved Pingora revision `518f7274eb9fbd68f3f55e85b8c86a9e4a59e95e`
  has `HttpPeer::hash` include `group_key`. Its BoringSSL `CertKey::hash` hashes
  the serial of each certificate, not the complete certificate/key material.
  A changed chain with the same serial sequence therefore needs Edgion's key.

The existing unsupported-rustls decision does not accept weakened BoringSSL
isolation. See the [upstream TLS architecture](../../../01-architecture/02-gateway/04-tls/02-upstream-tls.md).

## Alternatives and limits

- Fingerprinting borrows existing Secret data and retains only a digest; it
  neither logs nor serializes plaintext key material. Gateway fallback
  fingerprinting still happens during peer construction; this verification
  does not claim to have optimized that existing cost.
- Group keys and the final Pingora reuse hash are 64-bit. This is protection
  against omitted policy inputs, not a mathematical no-collision guarantee.
- Policy and Gateway sources are conservatively separated even for identical
  material. Unused Secret entries and SAN ordering can also separate otherwise
  equivalent policies. Extra separation costs reuse; it does not bypass trust.
- A new policy prevents new lookups from selecting the old pool bucket. It
  does not terminate existing streams or revoke requests holding an old
  immutable policy snapshot, and it does not detect changes to the host's
  system CA files under an unchanged `use_system_ca` setting.
- The private-key-only unit cases deliberately use mismatched cert/key pairs
  to isolate the hash input. They do not prove handshake acceptance of such
  identities. Network verification uses valid CA certificates.

## Verification and re-evaluation

Tests in `edgion-gateway/tests/unit/backends/peer_builder_test/pool_tests.rs` cover CA
mode/content, SAN composition, same-serial leaf replacement, same-length and
same-serial chain replacement, private-key-only changes, Gateway identity
rotation/removal, source separation, stable inputs, and policy precedence.
They assert the final Pingora reuse hash, with a negative control showing the
collision when only Edgion's `group_key` is cleared.

`Gateway/TLS/BackendTLS` includes `backend_tls_pool_isolation_on_ca_reload`.
It observes the backend's actual client socket to prove keepalive reuse, adds a
valid CA to the trust bundle while preserving address/SNI and requiring a new
connection, removes the original trust anchor and requires rejection, then
restores trust and verifies recovery. It runs in the shared Controller/Gateway
topology. `configclient/list` confirms configuration receipt only: a separate
conf-sync task debounces changes before publishing the runtime policy. After
that receipt barrier the test polls business outcomes for at most 15 seconds,
continuously exercising the connection so a missing discriminator cannot pass
merely because the old pool entry expires while idle. It requires a different
socket that is itself reused, then consecutive rejections after removing trust.
This is an HTTP/1 CA rotation test, not live HTTP/2 or mTLS identity rotation
coverage; those consumers additionally have source-level and hash evidence.

Verified on 2026-09-08:

- `cargo test -p edgion-gateway --lib pool_isolation`: 3 passed. The initial
  fixture incorrectly assumed that changing chain length left Pingora's own
  serial hash unchanged; the corrected same-length fixtures passed.
- The first live run caught the test's premature configclient receipt barrier.
  After replacing it with bounded business polling and rebuilding `test_client`,
  `run_integration.sh --no-prepare -r Gateway -i TLS/BackendTLS` passed all 6
  cases, including the CA reload case in 3.19 seconds. The report at
  `tmp/integration_testing/testing_20260908_165134/report.log` recorded PASS,
  including metrics, unknown-field, requeue-loop, and cache-residue gates.
  The preceding canonical prepare built the shared binaries and required test
  executables; the final run skipped only that already completed build step.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` completed successfully. Clippy
  reported existing warnings outside the changed code, with no new warnings
  in the pool-isolation tests.
- Agent-doc validation, SSA-force, metrics-inventory, and Pingora-source
  guards passed. No full-workspace test run was requested or performed.
- Independent read-only review returned LGTM after the timeout cleanup and
  asynchronous publication synchronization corrections.

Kubernetes runner verification on 2026-09-09 found a separate test-input defect:
the single and batch runners did not select endpoint inventory for
`Gateway/TLS/BackendTLS`. Without an explicit endpoint override, the pool test
used Service DNS in an IPv4 EndpointSlice, which Kubernetes rejected before the
TLS assertions. Both runners now reuse the existing Pod IP inventory for this
suite and preserve explicit overrides. A Gateway-only shell regression covers
the batch selection, including whitespace around the resource/item separator.
After that correction, the batch and single runners each passed all 6 BackendTLS
cases without a manually supplied endpoint IP. No production TLS behavior or
pool-isolation assertions changed. The cluster/image/CRD-overlay limits of this
run are recorded in the [canonical API verification](../../controller/regression-guards/backend-tls-policy-canonical-api-ingress.md).

Re-evaluate when TLS input fields, identity precedence, profile serialization,
policy publication, peer constructors, or Pingora hashing/pooling change.

## Resolution references

- `backend-tls-policy-connection-pool-isolation` — original pending P0 task
  dated 2026-08-30; original defect already corrected, regression verification
  completed on 2026-09-08. Production TLS code was not changed by this closure.
- `g7-006` — historical May 2026 review of SAN-only `DefaultHasher` and legacy
  endpoint port sentinels. The hasher discussion did not establish complete
  trust/identity coverage and is superseded by the current profile mechanism.
  The separate historical endpoint findings cited the removed `endpoint/`
  implementation (removal commit `43acc97`); that history is not fresh evidence
  about today's EndpointSlice port resolution.
