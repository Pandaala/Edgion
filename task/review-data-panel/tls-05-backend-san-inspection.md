# Backend SAN inspection without full display projection

Status: deferred extraction and matching boundary decision for the user.
Source baseline: 5f4dc4fa0. No runtime or dependency change.

## Evidence

The prepared BoringSSL backend handshake hook calls `evaluate_sans` in
`edgion-gateway/src/backends/prepared_backend_tls.rs`, which extracts an owned
SAN display vector before invoking `tls/validation/backend_san.rs`.
The shared certificate extractor enumerates ordinary SANs and then runs a
separate DER/OtherName decode pass. It formats DNS, IP, email, URI and supported
OtherName entries even though backend policy variants are only Hostname and URI.
Client certificate logs and typed mTLS consumers also use this extractor and
still need their established full projections.

The backend-independent outbound rustls client verifier separately parses
end-entity DER and materializes DNS/URI/IP/email strings before invoking the
same validator. It does not extract OtherName. Its base verifier runs first,
and empty policy skips extra extraction. This is not the unsupported Pingora
Gateway rustls backend and grants no work on that backend.

The RFC 6125 matcher already performs direct case-insensitive ASCII comparisons
and single-label wildcard checks without allocating or compiling regexes. The
candidate is avoiding intermediate projection, not introducing a second matcher
or precompiling an already allocation-free comparison.

## Equivalence boundaries

Backend validation currently consumes display strings. Hostname matching uses
the canonical typed-prefix exclusion; URI matching strips the literal `uri:`
prefix. For example, an ASN.1 DNS value beginning with `uri:` would participate
in URI matching under the current representation. A strictly typed visitor is
therefore not automatically equivalent. This source-level distinction is not
a demonstrated accepted certificate or live trust-bypass claim. Preserve the
current behavior or explicitly approve a separate semantic correction.

Policies are traversed before certificate strings, and the first matching policy
type selects the static success reason. Reversing the loops may change that
reason when both policy types match, even if the boolean result is unchanged.
Preserve ANY-match behavior, empty constraints, absent values, exact URI text,
case and wildcard behavior, and the existing static failure reasons.

The BoringSSL extraction and validation run inside catch_redacted_panic; skipping
work changes which failures can occur before a result. Preserve or explicitly
review the failure boundary when omitting the OtherName pass. The outbound
DER parser has a separate parse-error contract and does not use that same native
extraction path. Do not silently unify those differences or retain borrowed
certificate slices beyond their parser/native owner.

## Alternatives and decision

- Retain the full display-vector path until the interface scope is approved.
- Add a synchronous backend inspection projection to the canonical extraction
  boundary, paired with shared policy evaluation that preserves current semantics.
  Select projection before materializing owned outputs. Keep full client/log and
  typed mTLS projections unchanged, and do not add a competing SAN decoder.
- Separately design strictly typed backend policy matching if its semantics are
  desired; do not hide that behavior change inside an allocation optimization.

Measure intermediate vector/string allocations and irrelevant decode work under
representative certificates. Native SAN stacks and DER parsing may still allocate;
this does not promise allocation-free handshakes or throughput improvement.
This task authorizes no implementation, fork change or trust-policy expansion.

## Acceptance for an approved implementation

Cover mixed SAN types, duplicates, prefix-shaped DNS values, policy order,
empty/malformed fields, OtherName supported/unsupported encodings, invalid IP
lengths, missing SANs and parser/panic failures. Preserve chain/expiry/hostname
verification, immutable policy publication, backend pool isolation and handshake
result propagation. Keep diagnostics static and use existing access-log ownership.

Start from `skills/04-review/tls/regression-guards/typed-san-display-prefix-not-dns.md`
and `skills/04-review/tls/regression-guards/backend-tls-pool-isolation.md`.
Freeze the actual extraction, validation, caller and test allow-list only after
reviewing both supported backend and outbound-client paths.

## Current verification

Fresh baseline passed two prepared-backend, ten backend-SAN, 18 certificate
extractor and 12 outbound-verifier tests (42 total), formatting check, workspace
check/Clippy and agent-doc validation. Existing unrelated warnings remain.
These checks cover current behavior, not a future borrowed inspection API.
No full suite, live TLS integration or throughput benchmark ran.
