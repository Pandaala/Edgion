---
name: downstream-ecdh-policy-ownership
description: One EGC generation owns downstream ECDH groups for every certificate source.
knowledge_type: regression-guards
---

# Downstream ECDH policy belongs to EdgionGatewayConfig

## Unified downstream TLS policy

`ResolvedGatewayConfig.downstream_tls` owns `DownstreamTlsPolicy`: the EGC
minimum, shared cold `CipherPolicy`, and existing numeric `CurvePolicy`.
`CompiledEdgionTls` uses the same cipher compiler. Absent ciphers inherit;
explicit `[]` means backend defaults; a rendered CString or rejection remains
immutable. Resources/Controller validate NUL and curve syntax without linking
BoringSSL. The strict backend cipher setter validates effective expressions.

The early callback first selects the native, EdgionTls or exact frontend-mTLS
overlay identity, then applies policy once through `apply_selected_downstream_policy`.
Local minimum/ciphers replace EGC defaults independently, including a lower local
minimum or a valid local cipher masking an invalid default. Curves have no local
override. Unspecified ciphers inherit active FIPS/backend defaults; an explicit
empty list bypasses only the cipher default, never FIPS curve restrictions.
Any effective-policy error emits one canonical SSL audit and returns select error.
Certificate callbacks configure identity/mTLS only and never reload/reapply policy.
The local EGC Arc lives through application; SSL owns the copied settings afterward,
so no redundant EGC Arc is stored in SslCtx. Identity Arcs retain their existing lifetime.

No Rust handshake cipher joining, parsing or temporary cipher allocation occurs.
BoringSSL's public strict cipher setter internally parses its string; this existing
backend cost remains accepted. No SSL_CTX swapping, new locks or stores are added.
TLS1.3 suites and physical listener ownership/retargeting boundaries remain unchanged.

## Contract and evidence

`EdgionGatewayConfig.spec.downstreamTls.ecdhCurves` is the sole operator field.
The Resources validator bounds the ordered unique vocabulary to X25519/P-256/
P-384/P-521. Admission rejects malformed arrays before publication. If invalid
configuration reaches Gateway compilation, it becomes rejection, never absence. Gateway
listeners and EdgionTls have no curve override, parser or policy projection.

`ResolvedGatewayConfig` cold-compiles numeric group IDs. The early ClientHello
callback loads its existing startup-bound handle once and applies that policy
after identity selection and before protocol negotiation. SNI selects certificates, not curves.
No Edgion string parser/list clone/lock is added to handshakes; BoringSSL copies the
bounded numeric slice per connection. Existing certificate and frontend-mTLS
precedence remain unchanged.

Explicit FIPS-compatible subsets/order are retained; X25519 fails closed under
active FIPS, and absence uses the static NIST profile. Certificate callbacks cannot
broaden curves later. Failure uses the canonical single SSL audit and abort path.
Curve policy does not itself disable TLS1.2 static-RSA suites: negotiation tests
must require an ECDHE suite when checking disjoint groups.

## Verification boundaries

Resource tests cover syntax and absence versus invalid arrays. EGC publication
tests cover compiled rejection/default states, full/partial/delete generations
and retained snapshots. Runtime tests cover exact numeric IDs, FIPS constraints,
setter errors and actual TLS1.2/TLS1.3 negotiation over in-memory transports.
`DownstreamEcdhCurves` uses the shared topology to check the same policy across
native/EdgionTls SNI names, valid EGC updates, HTTP400 rejection of malformed
updates with the previous generation retained, field removal, recovery and
an established TLS connection. Test existence is not evidence of a live pass;
fresh results belong to the implementation verification record. Malformed runtime
generation rejection is covered by unit tests, not injected by the dynamic scenario.

Existing sessions do not renegotiate. The physical listener keeps its startup
owner's EGC handle even when another class shares that socket. GatewayClass
retargeting/ownership transfer, upstream TLS and PQC groups are outside scope.
Re-evaluate when these boundaries, EGC generation ownership or BoringSSL group
APIs change.

## RRC-001: bilingual FIPS curve documentation

The English and Chinese FIPS operator guides now describe the same explicit
curve contract: NIST subsets retain their requested order, while X25519 rejects
handshakes under active FIPS policy. Cipher overrides do not relax that boundary.
This reconciles RRC-001's documentation omission; it adds no FIPS compliance or
live-negotiation verification claim.
