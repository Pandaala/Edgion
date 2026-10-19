---
name: typed-san-display-prefix-not-dns
description: ClientCertInfo.sans and backend SAN display strings keep type prefixes; DNS/hostname consumers must use the shared prefix classifier so OtherName cannot be treated as DNS.
knowledge_type: regression-guards
---

# Typed SAN display prefixes are not DNS

**Status:** ✅ fixed · 2026-09-04
**Severity:** High (type confusion) on plugin `sanType: DNS` and backend Hostname pinning.

## Conclusion

Handshake mTLS matching uses ASN.1-typed `PresentedSan`. Display strings on
`ClientCertInfo.sans` and backend SAN lists stay prefixed for plugins and logs:

- DNS — unprefixed
- IP — `IP:`
- URI — `uri:`
- Email — `email:`
- OtherName — `otherName:<oid>=<value>`

Any consumer that treats "no `IP:`/`uri:`/`email:` prefix" as DNS must also
exclude `otherName:`. The single classifier is
`edgion_common::allowed_san::has_typed_san_display_prefix`. Re-reports that add
a local prefix list beside that helper are not accepted.

Plugin `SanType` still has no IP/OtherName variants; those identities remain
handshake `allowedSans` only. Untyped `mtlsMatch` field `san` still sees the
raw prefixed strings.

## OtherName decode

OtherName values are decoded from a complete BER object. `x509-parser`
leaves the RFC 5280 `[0] EXPLICIT ANY` wrapper in the value bytes; unwrap
that context-specific constructed tag 0 once (primitive `[0]` is skipped),
then stringify. A Universal BER object without that wrapper is skipped.

BOOLEAN, INTEGER/ENUMERATED (decimal, INTEGER capped at 64 bytes), OBJECT
IDENTIFIER, NULL (empty string), and string types including BMP/Universal
(embedded NULs are kept; do not truncate at `\0`). OCTET STRING, BIT STRING,
RELATIVE-OID, nested extra tags, incomplete BER, and raw UTF-8 are skipped.

## Requested projection ownership (ARCH-042)

`tls/runtime/backend/cert_extractor.rs` owns one private SAN enumeration with
separate display and typed projections. Select the projection before materializing
owned results: display extraction must not populate a discarded `PresentedSan`
vector, and typed extraction must not populate discarded prefixed strings.
Borrowed DNS/email/URI values are consumed synchronously. IP addresses remain
typed until projection: display formats the prefixed address directly, while
typed output owns its canonical address String. OtherName text values borrow
their BER input through synchronous projection; boolean and NULL values borrow
static text. Numeric, OID, BMP and Universal conversions retain owned results.
Display projection avoids the former temporary text/boolean String, while typed
projection owns borrowed values and moves already-owned conversions. NULL did
not allocate before this change. OtherName OIDs borrow the parsed object through
synchronous projection. Display formats that OID directly; typed output owns its
formatted OID. This removes the display path's outer temporary OID String, while
asn1-rs 0.7.2 Display still allocates internally through to_id_string. The BER
decoder remains shared. This changes ownership cost without changing trust policy
or merging the ordinary BoringSSL and OtherName DER passes.

The mirrored `tests/unit/tls/runtime/backend/cert_extractor_test.rs` contains
real signed mixed-SAN certificates with independently specified outputs for both
projections: DNS and OtherName duplicates, IPv4/IPv6, email/URI, multiple OIDs,
UTF8/BMP embedded NULs, unsupported OtherName values, invalid IP lengths and absent
SANs. OtherNames remain appended after ordinary entries even when interleaved in
certificate DER. Decoder regressions retain complete-input, wrapper and scalar
boundaries. These tests cover extraction contracts; they are not performance
measurements or a live TLS handshake/integration probe.
An input-pointer regression verifies borrowing for UTF8String with an embedded
NUL. Final projected values remain owned independently of the local DER buffer.


## Live probe evidence boundary (ARCH-352)

The BackendTLSSAN negative integration case requires both its original 5xx
response and the same request's canonical `error/BackendTlsSanValidationFailed`
diagnostic. A fresh W3C trace marker and existing test-store projection prevent
an unrelated backend failure or stale record from establishing SAN enforcement.
The positive case, fixture SAN constraints and production matching stay unchanged.

The negative exchange/body read has a ten-second budget, complete log polling
five seconds, and exact-marker deletion five seconds, within the 30-second case.
Observation or cleanup failures fail the case. Deletion is a bounded attempt;
outer cancellation or a late terminal record can outlive it. Pure verdict tests
cover unrelated 502/503, absent/wrong-kind diagnostics, wrong trace and successful
HTTP responses. They do not establish a fresh live TLS integration run.
