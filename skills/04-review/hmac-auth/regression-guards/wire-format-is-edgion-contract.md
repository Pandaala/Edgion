---
name: wire-format-is-edgion-contract
description: Use when reviewing HmacAuth request-format findings; the user guides document Edgion's runtime contract, not Kong, APISIX, draft-cavage, RFC 9421, or Envoy encoding.
status: fixed
finding_id: D0047
closed: 2026-09-05
knowledge_type: regression-guards
---

# HMAC Auth Wire Format Is Edgion's Contract — FIXED

## Conclusion

HmacAuth accepts `Authorization` or, if that header is absent, `Proxy-Authorization`, with scheme `hmac` and a `username` parameter. User guides must describe that contract. Feature-level alignment with other HMAC gateways (timestamp window, signed components, optional body digest) is enough; wire encoding does not have to match another product.

Fix suggestions of the form "document or parse `Authorization: Signature keyId=...`", "accept a standalone `Signature` header for authentication", "implement RFC 9421 `Signature-Input`", or "make the header encoding Kong/APISIX/Envoy-identical" are **not accepted**.

## Core Rationale

**1. Runtime contract is already frozen**

`parse_authorization` requires scheme `hmac` (ASCII case-insensitive) and quoted `username`, `algorithm`, `headers`, and `signature` parameters. Verification reads `authorization`, then `proxy-authorization`. A standalone `Signature` header is never an authentication source. After a successful authentication, `hideCredentials` removes `authorization`, `proxy-authorization`, and `signature` before proxying so leftover draft-cavage headers are not forwarded. The anonymous missing-auth path does not run this cleanup. Header names in the `headers` parameter are lowercased before the signing string is built.

**2. Industry products share the feature, not one encoding**

Kong hmac-auth happens to use `hmac` + `username`. APISIX and draft-cavage use `Signature keyId=...`. RFC 9421 uses `Signature-Input` / `Signature` structured fields. Envoy has no inbound HMAC request-auth filter. Copying any one of those encodings would either break existing Edgion clients or imply a compatibility surface the runtime does not provide.

**3. Documentation is the correction surface**

`docs/en/user-guide/http-route/filters/edgion-plugins/hmac-auth.md` and the Chinese counterpart describe Edgion's request format, signing-string construction, and the narrow `hideCredentials` strip of `Signature`. They do not claim HTTP Signature RFC compatibility.

## Fix Suggestions Not Accepted

- Add `Signature` / `keyId` parsing or aliases — expands the auth surface and contradicts the frozen parser
- Document draft-cavage or RFC 9421 as the Edgion request format — clients following that text receive 401
- Change header precedence, algorithm set, or signing-string rules to match Kong/APISIX — those are implementation details, not product requirements
- Edit frozen HmacAuth integration tests as part of a documentation alignment — runtime behavior is unchanged

## Re-evaluation Triggers

Re-open this decision only if:

- The runtime parser is intentionally changed to accept another wire encoding
- Edgion adds a separately designed RFC 9421 or draft-cavage authenticator

## Reference Cases

- D0047 / `tasks/todo/align-hmac-auth-wire-format-documentation.md` (closed by this documentation alignment)
- `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs` (`parse_authorization`, `verify_headers_with`, `hide_credentials`)
- `edgion-tests/src/client/suites/edgion_plugins/hmac_test_utils.rs`

## Resolution references

- `D0047` — 2026-09-05.

## Borrowed quoted-parameter decoding

`parse_quoted_value` borrows the trimmed quoted interior when it contains no
backslash. Values requiring unescaping retain the original character decoder and
owned output. The local parameter map holds those borrows only during synchronous
parsing; the returned username, signed-header names and decoded signature remain
owned before verification or asynchronous body capture begins.

This changes temporary ownership only. Outer quote checks, permissive inner quote
handling, arbitrary escaped characters, trailing-backslash rejection, lowercase
parameter keys and signed-header duplicate rejection retain their current behavior.
Parameter keys without ASCII uppercase bytes also borrow their trimmed input;
keys requiring ASCII folding retain owned lowercase storage. The map hashes and
compares the resulting str values identically, including non-ASCII bytes.
Unknown parameters are still accepted after validating their quoted values;
duplicate keys still fail before parsing the duplicate value. This is not a new
wire format or a relaxation of authentication, timestamp or body-order checks.

Unit regressions verify zero allocations and source-pointer identity for unescaped
quoted interiors, escaped and malformed cases, owned results after source release,
and duplicate/unknown-parameter error precedence. Other parser structures and final
results still allocate; no whole-parser allocation or throughput claim follows.

## Borrowed current query in signing targets

Signing-string construction uses the existing `request_query` borrow instead of
copying `get_query`. Both accessors read the current request URI query. This is
not the decoded, frozen entry-query cache: trusted URI rewrites remain visible,
and percent encodings, plus signs, duplicate keys and empty values stay raw.
Both request-target spellings omit the question mark for absent or empty queries.
Path, method and header ordering, final signing-string ownership and digest bytes
remain unchanged.

The signing-target matrix forbids the owned accessor and checks absent, empty and
encoded queries for both spellings. Existing fixed signing/HMAC vectors retain
wire-byte coverage. A real-adapter regression changes the URI repeatedly and
checks pointer identity, owned-accessor parity and zero query-read allocations,
while separately confirming the frozen decoded entry cache. This removes a query
copy per target component; constructing the signing string still allocates.

## Allocation-free algorithm-name selection

The canonical resources-layer `HmacAlgorithm::parse` compares the input with the
three fixed `as_str` names using ASCII-insensitive comparison. It no longer builds
an intermediate lowercase String. The supported set, ASCII case behavior, lack
of whitespace trimming and rejection of unknown/non-ASCII names remain unchanged.
Serde enum decoding and the wire schema are unaffected; this helper is called by
the Gateway Authorization parser, which retains its existing error order.

Resources unit tests exercise every ASCII case permutation of the three names
and malformed boundaries. The Gateway allocation regression checks zero allocated
calls/bytes for selected valid and invalid names. These checks cover algorithm
selection only; the complete Authorization parser still owns its other outputs.
