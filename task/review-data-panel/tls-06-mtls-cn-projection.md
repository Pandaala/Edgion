# Evaluate minimal CN inspection for the mTLS allowlist

Status: deferred user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `dbc7f6190`. No runtime implementation selected.

## Evidence

In `tls/boringssl/mtls_verify_callback.rs`, a nonempty allowed_cns list causes
validate_certificate_whitelist to build a full ClientCertInfo through the
canonical cert_extractor before invoking validate_cn_whitelist. The validator
only reads cn and performs exact case-sensitive equality against the list.
Full extraction also renders every subject entry, enumerates display SANs,
serializes DER for the separate OtherName parse pass, and computes/formats the
SHA-256 certificate fingerprint. Those additional values do not decide CN success.
On CN rejection the diagnostic reads subject and cn, applies sensitive masking,
and never emits the other extracted fields.

Missing certificates reject before constraint checks. SAN validation precedes CN
validation; SAN-only success and absent/empty constraints already avoid full
metadata extraction. A successful SAN check followed by a nonempty CN constraint
still enters full extraction. This proposal does not replace typed SAN matching.

The full extractor also serves handshake-completion metadata when exposure is
enabled (and Gateway frontendValidation mTLS), plus HeaderCertAuth certificate
input modes. Those consumers retain their full projection requirements. This is
not permission to remove fields globally or cache client identity between handshakes.

## Canonical decoding and ownership boundaries

The current canonical subject traversal selects the first entry whose short name
is CN. Its seen marker advances even if as_utf8 fails: an unsupported first CN
must not promote a later valid CN. Existing signed-certificate tests cover that
case and duplicate-CN ordering. Locked boring 5.2.0's OpensslStringRef conversion
uses CStr bytes, so subject/CN text truncates at embedded NUL. Do not accidentally
substitute the different OtherName string semantics, which preserve embedded NULs.

A minimal inspection must share the canonical first-CN/decode rule and keep the
native converted-string owner alive throughout any borrowed comparison. A second
ASN.1 decoder or a plugin-local approximation is not an acceptable shortcut.
The existing validator takes ClientCertInfo and is publicly re-exported; review
actual consumers before choosing a value-oriented matcher or extractor seam.
There is no requirement to keep an unused compatibility adapter after a reviewed
interface change, but no interface change is selected here.

## Alternatives and tradeoffs

- Keep the full projection until representative CN-constrained handshake costs
  establish a useful benefit.
- Add minimal CN inspection within the canonical extractor and a shared exact
  value comparison. Defer full diagnostic projection until rejection, while
  keeping handshake metadata and HeaderCertAuth on their existing full path.
- Evaluate sharing already decoded CN state with a later failure projection only
  if duplicate decoding is material; avoid retaining an extra full snapshot or
  adding cross-callback identity state merely to simplify mismatch handling.

Success could avoid unrelated SAN/DER/fingerprint and subject-rendering work;
failure may decode CN twice if it falls back to complete extraction. Measure both
accepted and rejected chains, varied subject/SAN sizes and exposure settings.
Borrowed comparison does not eliminate native as_utf8 allocation. Skipping other
projection work also removes its possible failures inside the redacted panic
boundary; review that distinction rather than claiming automatic equivalence from
matching ordinary certificates. Keep missing/invalid-CN fail-closed behavior.

This differs from prepared SAN policy in [tls-01](tls-01-prepared-san-policy.md),
connection snapshot handoff in [tls-04](tls-04-header-cert-connection-projection.md),
and backend SAN display inspection in [tls-05](tls-05-backend-san-inspection.md).
No rustls Gateway support, trust-policy expansion or new certificate cache is in scope.

## Acceptance before implementation

Compare current and candidate decisions/diagnostics for absent certificates,
absent/empty lists, absent/empty CN, duplicate CNs, unsupported first CN with a valid
later CN, supported ASN.1 string encodings, embedded NUL, exact case-sensitive
matches and literal wildcard text. Cover CN-only and combined SAN/CN policies,
including SAN rejection precedence and sensitive masking on both rejection paths.
Preserve leaf-only evaluation, chain validation, one terminal ssl.log claim,
FFI panic containment and connection-owned certificate lifetime. Preserve subject
ordering, placeholders, SAN projections and fingerprint output for full consumers.

Use the existing signed-certificate and callback tests and add distinguishing
projection/allocation evidence only for the selected design. Measure whole
handshake projection costs, native allocation and repeated mismatch work separately;
source-level avoided calls are not a throughput or total-memory result.
Start from the original `mtls-leaf-only-whitelist-verification.md` and
`typed-san-display-prefix-not-dns.md` under `skills/04-review/tls/regression-guards/`,
the downstream TLS architecture, and the canonical cert_extractor tests. Freeze
an exact implementation allow-list and obtain independent reviews before editing.
