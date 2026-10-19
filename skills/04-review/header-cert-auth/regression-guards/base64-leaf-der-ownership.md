---
name: header-cert-auth-base64-leaf-der-ownership
description: Base64 leaf ownership, shared direct-issuer verification and retained URL PEM constructor boundaries.
status: fixed
finding_id: ARCH-034
knowledge_type: regression-guards
---

# Header certificate format and leaf ownership

Base64 Header mode owns its decoded leaf DER through `verify_leaf_der` and
BoringSSL `X509::from_der`, then calls the canonical client certificate identity
extractor. ARCH-034 removed the former DER-to-PEM encoding, PEM text construction
and second leaf DER decode. This is structural evidence of removed work, without
measured allocation, memory, latency or throughput claims. The canonical
extractor's own SAN/OtherName DER work is separate.

The Base64 normalization input borrows the trimmed header when it contains no
ASCII whitespace. Otherwise it retains the existing character filter and owned
String. The byte scan detects exactly the ASCII characters removed by that
filter, preserving decoder input even for non-ASCII text. The no-whitespace path
avoids the temporary full-input copy; the whitespace path adds a scan through
the first whitespace. This is not a measured throughput improvement.

Both formats share the same private borrowed-DER validity, issuer-Name and
ordered direct signature verification loop. URL retains its percent-decoded PEM,
first eligible nonempty `CERTIFICATE` selection in `verify_against_ca`, and
`X509::from_pem` for identity. No new certificate framework, cache, trust policy,
public session capability or configured-CA owner is introduced.

## Preserved behavior

- Header trim, ASCII-whitespace filtering and strict STANDARD Base64 decoding.
- Format errors before empty-CA rejection; empty CA before leaf parsing.
- Empty decoded Base64 is `invalid-cert-pem`; malformed nonempty DER is
  `invalid-leaf-cert`; expired leaf is `expired-leaf-cert`; unmatched issuer or
  failed signature is `ca-verification-failed`.
- Constructor failure after successful verification remains `invalid-cert-pem`.
- Parser remainders and trailing DER are accepted as before; no full-consumption
  check was added. The locked boring 5.2.0 PEM and DER constructors ultimately
  use the same DER parser for a Base64 payload.
- Cold CA validity/provenance, ordered pool search, canonical identity fields,
  Connection mode, anonymous allowance, credential hiding, shared rejection
  delay/challenge and sensitive header writes retain their existing owners.

URL has a distinct parser boundary. BoringSSL accepts the older `X509 CERTIFICATE`
label that the private verifier skips; an older-label block before an eligible
block can produce identity from the first certificate while verifying the second.
The x509-parser PEM reader also does not compare END labels, while BoringSSL
rejects mismatched envelopes. These observed boundaries are retained here, not a
new approval to broaden trust. Replacing URL identity extraction with verifier
DER requires a separately scoped identity and envelope acceptance decision.
The existing [Header trust rationale](../review-evidence/chain-verification-header-mode.md)
applies independently.

## Verification evidence and limits

Production: `edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs`.
Unit controls: `edgion-gateway/tests/unit/plugins/http/header_cert_auth/plugin_test.rs`.
Tests exercise real Header extraction and compare all canonical identity fields;
cover spaces/tabs representable in HTTP headers, invalid/strict Base64, empty and
malformed DER, no-CA ordering, trailing DER, expired leaf, distinct issuer Names,
same-subject wrong-key rejection and later-valid CA success. URL controls use
certificates with different subjects/fingerprints for older-label identity
selection, ordinary/multiple blocks and mismatched END rejection.

Execution results belong to the current implementation record; this page does
not substitute historical results for fresh verification. These controls do not
prove TLS deployment trust, full chain construction or measured performance.
Re-evaluate on constructor/parser dependency changes, URL identity-selection
changes, full-consumption policy changes, or changes to header-mode trust scope.
