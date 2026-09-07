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
