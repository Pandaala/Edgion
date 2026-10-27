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
backslash. Values requiring unescaping copy unchanged spans directly and decode
the single Unicode scalar after each backslash into the same owned output.
ASCII delimiter positions and the decoded scalar's byte length keep slice
boundaries valid; a dangling backslash still returns the original error.
The parsed username retains that Cow through synchronous header
verification. Only a successful header-verification outcome takes ownership,
before asynchronous body capture or session mutation. Signed-header names and
the decoded signature remain owned parser outputs. This avoids copying an
unescaped username on synchronous rejection paths; escaped usernames still need
the decoder's owned output.

This changes temporary ownership only. Outer quote checks, permissive inner quote
handling, arbitrary escaped characters, trailing-backslash rejection, lowercase
parameter keys and signed-header duplicate rejection retain their current behavior.
Parameter keys without ASCII uppercase bytes also borrow their trimmed input;
keys requiring ASCII folding retain owned lowercase storage. The map hashes and
compares the resulting str values identically, including non-ASCII bytes.
Unknown parameters are still accepted after validating their quoted values;
duplicate keys still fail before parsing the duplicate value. This is not a new
wire format or a relaxation of authentication, timestamp or body-order checks.

Unit regressions verify zero allocations for unescaped quoted interiors, source
pointer identity for borrowed parsed usernames, escaped and malformed cases,
owned verified results after session release, and duplicate/unknown-parameter
error precedence. Other parser structures and final results still allocate; no
whole-parser allocation or throughput claim follows.

After all required values have been extracted in the original order, the parser
drops the remaining parameter map before algorithm/header conversion. Extracted
Cows own their buffers or borrow the original Authorization input, never the map.
The decoded algorithm and signed-header source values drop after conversion;
the duplicate-header set drops after validation and before signature decoding.
Unknown escaped values and folded keys can own buffers, while unescaped values
borrow input. Final username/header/signature ownership and every rejection order
remain unchanged. This shortens temporary storage overlap, without reducing the
number of parser allocations or establishing physical erasure, RSS or throughput
improvement. It changes neither the accepted credential-map ownership nor the
deferred signed-header membership representation. Source ownership and compilation
establish these release points; behavioral tests do not measure peak lifetime.

## Borrowed current query in signing targets

Signing-string construction uses the existing `request_query` borrow instead of
copying `get_query`. Both accessors read the logical current Query owner. This is
not a decoded single-key projection: trusted URI rewrites remain visible,
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

## Forward-only parameter segmentation

`split_auth_params` yields trimmed input slices through a forward-only iterator
instead of collecting a temporary Vec. Its quote/backslash state transitions,
empty and trailing segments, and permissive handling of unfinished quotes remain
unchanged; quoted-value validation still belongs to `parse_quoted_value`.
The sole production caller consumes segments in order and retains its existing
error precedence. Returning early skips only a side-effect-free tail scan.

A differential regression compares the previous scanner over all 9,331 strings
of length zero through five from a six-character alphabet, plus longer escaped
and whitespace cases. It measures zero allocation calls and requested bytes for
segment iteration only. The complete Authorization parser still allocates its
parameter map and owned results; this is not a throughput benchmark.

## Header-verification scratch release

After successful signature comparison, verify_headers_with releases the signing
String and decoded signature Vec before checking the existing signed-digest
requirement. After that check, it releases owned signed-header names before
converting the verified username Cow into its result owner. The username either
borrows the original Authorization value or owns independent unescaped text; it
does not borrow these released fields. No custom destructor is involved.

This shortens temporary overlap with successful username materialization. It
does not reduce allocation count, change the HMAC input, move body capture, or
change rejection order. Parser scratch, credentials, body-digest snapshots and
asynchronous body-read owners remain distinct. Rust drop is not physical erasure
and no throughput or overall peak-memory improvement is established.
