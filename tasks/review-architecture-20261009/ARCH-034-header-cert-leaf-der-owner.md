---
id: ARCH-034
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-321
independent_verification: VERIFICATION-ARCH-034.json
---

# Keep the decoded DER owner in Base64 header certificate authentication

## Current ownership and avoidable work

In HeaderCertAuth's Base64Encoded mode, decode_header_cert_to_pem decodes
the header into DER, then der_to_pem encodes those bytes back into Base64
and builds a PEM String. verify_against_ca parses that generated PEM into
another DER Vec for x509_parser signature/validity checks. extract_from_header
then passes the generated PEM to BoringSSL X509::from_pem for canonical
ClientCertInfo extraction.

This path starts with the DER representation both cryptographic consumers
can use, but creates a textual intermediate and another decoded DER owner
because the private helpers share a PEM input. The concrete avoidable cost
is re-encoding, text construction and re-decoding of the same Base64 leaf,
not a measured speedup or a certificate trust defect. The canonical identity
extractor's own SAN/OtherName work is separate.

## Bounded improvement and format boundary

Retain Base64 mode's decoded DER. After its format decode and the existing
empty-CA admission check, verify by borrowing those bytes and construct
BoringSSL X509::from_der from the same owner before calling the existing
extract_client_cert_info_from_cert function. Factor the existing leaf
validity/direct-issuer signature loop into one private DER-consuming helper
shared with URL mode; do not duplicate verification policy.

Retain URL mode's current percent-decoded PEM path, first eligible
CERTIFICATE selection for verification and X509::from_pem identity extraction.
A uniform one-DER-owner migration for URL input is not established as
behavior-preserving. The locked x509-parser PEM reader does not compare END
labels, while BoringSSL validates them. BoringSSL's PEM certificate reader
also accepts the older X509 CERTIFICATE label, which the private first-cert
selector skips. An earlier such block followed by an eligible CERTIFICATE
can make the existing verifier and identity extractor select different
certificates. Replacing the PEM constructor with the verifier's DER could
therefore change identity or turn a formerly rejected envelope into success.

These source-level discrepancies require a separately scoped acceptance and
identity decision before URL unification. They are not justification to
reimplement BoringSSL PEM grammar inside this optimization. Keep explicit
format branches and one borrowed DER verifier rather than a generic certificate
framework, new cache, public schema, session capability or parsed-CA abstraction.
The current cold CA subject/DER ownership remains intact.

## Required semantics and risks

Preserve header trimming, ASCII whitespace filtering and strict Base64
decode. Format-decoding failures precede the no-CA check; no-ca-configured
precedes leaf parsing or validity checks. After a nonempty trust pool is
established, empty decoded Base64 must still yield invalid-cert-pem, while
nonempty malformed DER yields invalid-leaf-cert. BoringSSL constructor
failures retain invalid-cert-pem after successful CA verification.

The locked boring 5.2.0 constructors both ultimately invoke d2i_X509 for
the same DER payload; their wrappers do not require that its input pointer
consume all trailing bytes. Preserve the current x509_parser remainder
handling and DER trailing-data behavior. Do not silently add a full-consumption
check. Constructor/input parity still needs real regression verification.

Keep direct issuer/CA signature filtering, per-request leaf validity, cold
CA validity filtering, ordered pool search and selected identity fields.
Do not add full path construction or change the original header-mode trust
boundary. Preserve Connection mode's owned ClientCertInfo use (ARCH-007),
anonymous/hide-credentials policy, shared delay/challenge, sensitive header
writes, fixed diagnostics and no payload/PII logging.

## Future verification and original evidence

Cover the real Base64 header extraction path with a valid leaf and identical
identity fields, ASCII whitespace, malformed/empty input, invalid Base64,
trailing DER, empty trust pool, issuer mismatch, expired leaf and a later
matching CA. Retain current URL behavior, including multiple blocks, older
labels and mismatched envelopes; do not infer URL parity from direct verifier
tests. Existing CA verification and prepared-pool tests are meaningful but
do not prove constructor/input parity or elimination of the textual round trip.

Original single-layer header trust, shared failure-delay and identity-header
records, actual private callers, canonical extractor, current CA tests and
locked Rust/C++ constructor and PEM-parser sources were inspected. No decision
requires Base64's textual intermediate. This is separate from ARCH-007's
borrowed connection-certificate conditions and leaves both cryptographic
libraries in their current roles. Static independent review only: no code
change, tests, parser differential execution, crypto oracle, allocation/RSS
measurement or throughput result.
