---
id: ARCH-117
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-426
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Publish the handshake verifier and its revocation token as one snapshot

## Current ownership and concrete interleaving

ServerIdentityHandle owns the current conf_sync TLS verifier/leaf and the cancellation token intended to govern sessions authenticated by its trust root. These are separate ArcSwap slots. ConfigSyncGrpcServer::serve_with_shutdown at grpc_server.rs:365–370 first constructs a TlsAcceptor from handle.load().config and then independently calls handle.trust_token(). ServerIdentityHandle::store at tls_identity.rs:154–162 installs the identity, swaps the token for a root change, and cancels the previous token.

A feasible interleaving uses only one writer: the accept loop loads root A's config; another runtime worker completes store of root B, swaps to token B and cancels token A; the accept loop then loads token B and starts its handshake using config A. An A-authenticated peer can complete against the retained A verifier and enter TrustScopedStream carrying uncancelled B's token. Completing the root change therefore fails to revoke this particular connection. There is no await between the reader's two loads, but they are separate instructions on a multithread runtime and are not one snapshot. Neither ArcSwap's individual atomic publication nor the diagnostic AtomicU64 couples those reads.

TrustScopedStream::poll_alive checks only its captured cancellation future; it does not consult verifier identity or trust generation again. The existing design deliberately has no blanket max_connection_age. Its handshake timeout bounds handshake time, not the resulting established session's lifetime. This is a statically established race, not an executed exploit or production incident. It is distinct from eventual per-replica root convergence: this witness occurs after this replica has completed its own store.

## Bounded improvement and change scenario

Give the installed identity and its governing trust token one private immutable publication owner inside ServerIdentityHandle. Expose a handshake snapshot that obtains both from one load, and have the accept loop construct its acceptor and TrustScopedStream token from that snapshot. On a same-root leaf rotation, publish a new identity retaining the same token. On a root change, publish the new identity with a fresh token and cancel the replaced root's token; retained pre-change snapshots must always point to a token that is cancelled by that change. No handshake can borrow an old verifier with a later root's live token.

Keep concurrent cold store updates coherent too: serialize the private compare/publish/cancel operation or use an equivalent reviewed compare-and-swap loop. Do not add a blocking read lock on the accept path. This is a publication invariant, not authority to repair every producer's stale-input decision. Keep existing producer generation/serial checks and treat any stronger stale-identity-write proposal separately. Identity metadata/expiry accessors can continue projecting the identity from the same installed owner; no extra PEM/key copies, generic certificate registry, observability or connection-age policy is needed.

The change scenario is an emergency re-key or BYO-root replacement overlapping a newly accepted TCP connection. One canonical handshake snapshot reduces the number of separately ordered operations each accept caller must get right. File-mode reload and leader/per-replica auto-CA stores must use the same handle boundary, preserving validate-before-publish and last-known-good behavior. This backend-independent Controller rustls transport is outside the unsupported Gateway rustls backend seam.

## Alternatives, invariants and verification

Reading the token before the config under a documented single-writer ordering can avoid the specific old-config/new-token interleaving, with possible conservative cancellation of a new-root handshake. It still exposes two independently published pieces of one trust invariant and would need a proof covering all concurrent writers. A single installed snapshot expresses the required pairing directly; compare its small cold-publication cost against the simpler ordering fix during implementation design. Merely checking trust_generation after both loads is not a fix without a complete retry/publication protocol.

Preserve same-root established-stream continuity, distinct-root and same-key-root-renewal cancellation, validated leaf/CA/key material, h2 ALPN, process/term cancellation, bounded handshake slots and handoff, explicit plaintext escape-hatch versus omission fail-close, binary-owned diagnostics, redaction and existing per-replica/leader ownership. No request body, response aggregation, resource workqueue or new logging/metric policy changes are proposed.

The actual tls_identity_test.rs same-root/root-change/renewal tests and grpc_server_test.rs real TLS leaf-rotation/root-change tests prove sequential intended contracts. They do not force a store between the accept loop's two loads. Future verification must deterministically retain a pre-change handshake snapshot across a root change and establish that its token is cancelled, demonstrate no old-verifier/new-live-token pairing, exercise concurrent handle writers, and preserve the existing real TLS tests for old-client refusal, fresh-client acceptance and same-root continuity. No tests were executed in this review.

## Original evidence and limits

The original Controller–Gateway overview at skills/01-architecture/03-controller-gateway-link/00-overview.md:196–222 explicitly requires root-change session teardown while retaining same-root sessions. The source store comment claiming any racing handshake must use the new config is contradicted by a pre-store config load. The accepted metadata-signature watcher tradeoff governs whether reload is triggered, not atomic handshake pairing once a root store happens. Transactional Controller channel selection governs the Gateway's installed channel and cannot prove this Controller accept invariant. ARCH-075 concerns certificate selection before installation; ARCH-074/080 concern security configuration selection. None reports this handle/accept pairing race.

Production callers inspected include cert_watch::reload_from_files and both auto-CA store sites in identity_sync.rs and rotation.rs. The witness needs only one concurrent store; it makes no claim about their wider stale-source ordering, absolute process-wide revocation deadlines, deployed compromise, or measured performance. Review remains source-only and read-only.
