---
id: ARCH-017
status: candidate
kind: architecture-debt
priority: P2
primary_partition: PART-251
independent_verification: confirmed
---

# Prepare immutable native backend TLS components with their publishing generation

## Evidence and ownership cost

backends/peer_builder.rs:110–279 derives stable TLS material at every actual peer
construction. build_client_cert_key_from_secret parses certificate chain/private
key and creates an Arc<CertKey>. create_tls_peer repeats it for a policy identity
or selected Gateway fallback; build_ca_chain reparses custom CA PEM; the SAN
closure clones configured names; backend_tls_group_key sorts and fingerprints
the selected Gateway Secret's full data again. Service and ExternalName selection
reach this builder, as does AI EdgionBackend dispatch. It runs before determining
whether a connection can be reused.

The effective policy owner already publishes raw material with a complete profile
digest and reuses unchanged effective Arcs. Gateway identity publication retains
Arc<Secret>. Neither publishes the immutable transport components corresponding
to that material. A new trust source or identity check must consequently coordinate
cold profile preparation, request-time parsing, fallback parsing and pool identity.
Current parsing and fingerprinting use a coherent captured Secret: this finding
asserts repeated work and split preparation ownership, not a rotation defect.

## Bounded improvement and alternatives

Extend those existing publication owners with immutable BoringSSL preparation
results. A policy generation carries prepared identity, complete custom CA chain,
immutable SAN policy components/closure and its existing complete identity. A
Gateway fallback generation carries prepared identity and full Secret-data digest
as one value. Actual peer construction combines the captured preparation with
endpoint, SNI and options, using Arc clones and bounded digest combination.

Retain an explicit preparation failure in the generation: malformed material must
fail closed when used, without withdrawing it and activating system trust or another
identity. Only immutable policy components are shareable. Each handshake still
inspects its own peer certificate and creates its own BackendSanValidationResult;
SSL state, observations and digest extensions remain per connection.
pg_connected_to_upstream retains current rejection and pool reuse semantics.

Caching whole peers would couple address/ALPN/timeouts incorrectly; a lazy global
cache adds independent invalidation and synchronization. Generation preparation
retains native crypto objects while captured generations live and moves work to
publication, so retained memory and cold failure representation need review.
Observed work is repeated PEM parsing, material-proportional hashing, CA allocations
and SAN closure allocation; no latency/allocation benchmark was executed.

## Invariants and future checks

Preserve identity/trust precedence, error classes, no-op publication reuse,
target-local atomic publication, captured old-policy lifetime, live Gateway
fallback selection, multi-PEM trust and complete material-dependent pool isolation.
BoringSSL adaptation legitimately differs from Common's rustls client builder.
Independent active-health transport remains separate. General OutboundTlsLocal
adaptation stays with Common's canonical pure resolver and explicit defaults.
No rustls Gateway support, new general resolver or request diagnostic logging.

Future native builder/pool tests should verify repeated peers do not reprepare,
no-op publication retains component Arcs, material rotation changes reuse identity,
malformed replacements fail closed, policy identity overrides fallback, plaintext
without a policy performs no fallback work, and shared SAN preparation produces
independent handshake verdicts. Preserve address/timeouts/appProtocol/retry/logging.

## Original authority and limits

The analyst fully read eight backend primaries, policy publication and Gateway
startup seams, actual HTTP/AI/TCP/TLS callers, native builder/pool tests and original
backend-tls-pool-isolation, backend-client-identity-tls-selection,
backend-ca-bundle-multi-pem, build-client-cert-key-dedup,
build-peer-tls-helper-dedup and incremental-atomic-publication records.
Pool-isolation explicitly makes no claim of optimized Gateway fingerprint cost;
parser deduplication requires one helper, not reparsing per request.
No current task accepting this preparation cost was found. Static source review;
independent verification is pending.
