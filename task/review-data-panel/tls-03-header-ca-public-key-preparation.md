# HeaderCertAuth CA public-key preparation

Status: deferred ownership and parser-equivalence decision for the user.
Source baseline: b06f91f42. No runtime change.

## Evidence

HeaderCertAuth::load_ca_certs in
edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs parses each CA and
checks its validity at construction. ParsedCa retains its subject DER and full
certificate DER. On each Header-mode request, verify_leaf_der parses the leaf,
checks its current validity, and compares its issuer bytes with each retained
subject. Only matching entries reparse the complete CA to borrow its public key
for leaf.verify_signature. Same-subject wrong keys can cause several parses
before a later valid CA succeeds. Nonmatching entries do not parse CA DER.

The workspace dependency is x509-parser 0.18.1 with verification enabled.
SubjectPublicKeyInfo contains borrowed algorithm parameters, key data and raw
DER; cloning it does not make the lifetime independent of its backing bytes.
The verifier selects its algorithm from the leaf signature algorithm and, where
needed, the CA key algorithm parameters. Preparing one algorithm solely from
the CA key would not preserve that dispatch. No replacement crypto verifier is
proposed.

## Parser boundary

Copying SubjectPublicKeyInfo::raw into an owned Vec and reparsing it is not a
proven equivalent replacement. In the locked parser, raw ends at the cursor
after the algorithm and BIT STRING. The enclosing der-parser 10.0.0
parse_der_container ignores the inner parser remainder while consuming the
complete declared container from its caller.

A standalone probe linked against the local locked x509-parser artifact used
this synthetic SPKI (hex):

    30 0c 30 04 06 02 2a 03 03 02 00 01 05 00

SubjectPublicKeyInfo::from_der accepted all 14 bytes with no outer remainder,
returned a 12-byte raw slice, and reparsing that slice failed: its sequence
header still declares the original content length. This is a generic parser
ownership counterexample, not a signed-certificate acceptance test or evidence
of a trust bypass. Existing certificate tests do not establish equivalence for
this boundary. Do not silently tighten accepted inputs as a performance change.

## Decision alternatives

- Keep the current implementation pending design and workload evidence. This is
  temporary deferral, not a permanent exemption from cold preparation rules.
- Retain the exact enclosing SPKI TLV with checked bounds and parse only that
  smaller object on issuer match. Prove extraction and acceptance equivalence,
  including ignored inner bytes and algorithm parameters; do not equate raw
  with the entire TLV. This still performs request-time SPKI parsing.
- Prepare an independently owned parsed-key representation, or a carefully
  reviewed lifetime owner for borrowed parser state. Compare retained memory,
  cold-copy cost, drop behavior and parser coupling. Do not leak backing storage,
  introduce unsafe lifetime extension, duplicate algorithm policy or add a
  global certificate cache merely to avoid the existing parse.

Measure issuer-match frequency, same-subject pool width, certificate sizes and
parse/verification cost before claiming CPU or throughput gains. Full parsed
certificate retention may cost more than the current DER storage; a smaller
owned key representation needs an explicit field/lifetime design. Any chosen
implementation needs a separate IMPL plan and independent review.

## Acceptance for a selected implementation

Preserve cold CA validity/provenance checks, per-request leaf validity, exact
issuer/subject byte comparison, original pool order and later-valid same-subject
fallback. Keep leaf signature algorithm dispatch and existing verification API
semantics, including unsupported parameters. Preserve no-CA and format-error
precedence, trailing DER handling, anonymous behavior and credential hiding.

Both Base64 and URL modes share the verifier, but retain their distinct identity
constructors and the documented URL PEM selection/envelope boundaries. Connection
mode remains unchanged. Keep immutable plugin-generation ownership, Secret reload
behavior and canonical client-certificate identity extraction. This task grants
no full-chain validation, trust-policy, logging, metric or schema change.

Compare prepared and current verification across supported key/signature families,
wrong and same-subject keys, malformed/extra ASN.1 fields, expiry and pool reloads.
Include retained-storage and request-allocation evidence where claimed. The
existing 20 HeaderCertAuth tests are a baseline, not comprehensive evidence for
these future alternatives.

Relevant original knowledge:
[leaf ownership](../../skills/04-review/header-cert-auth/regression-guards/base64-leaf-der-ownership.md)
and [Header trust boundary](../../skills/04-review/header-cert-auth/review-evidence/chain-verification-header-mode.md).
