---
name: session-cookie-upstream-ownership
description: Use when reviewing whether OpenID Connect session or state cookies should reach upstream; they are Gateway-owned and always stripped before proxying.
status: fixed
finding_id: D0060
closed: 2026-09-04
knowledge_type: regression-guards
---

# OpenID Connect session/state cookies are Gateway-owned — FIXED

## Conclusion

`edgion_oidc_session` (or `sessionCookieName`) and `edgion_oidc_state` are Gateway credentials. Every request path that continues upstream strips both names through `auth_common::strip_cookies` before proxying. `hideCredentials` remains bearer-`Authorization` only.

Both names are removed in one atomic operation. Successful session verification,
refresh, leeway fallback, direct token authentication and unauthenticated Pass
all propagate cleanup failures: opaque non-UTF-8 Cookie fields return 400; internal
mutation failures return 500. Both use the existing runtime response dispatcher;
none continue upstream with the unsanitized original request. No match preserves
raw fields; actual removal may normalize separators and combine field lines.

Fix suggestions of the form "gate session-cookie removal on `hideCredentials`" or "leave the encrypted session cookie visible to the application" are **not accepted**.

## Core Rationale

**1. The cookie is a replayable Gateway credential**

Successful session-mode authentication previously left the AEAD-encrypted session cookie on the proxied request. Ciphertext is still replayable against the Gateway. Refresh tokens may still sit in that cookie.

**2. Aligns with Envoy flow-cookie ownership, not Traefik pass-through**

Envoy's oauth2 filter unconditionally removes HMAC / nonce / refresh / code-verifier cookies before upstream and keeps token forwarding on dedicated knobs. Kong's `hide_credentials` defaults to stripping used credentials. Traefik's default pass-through is the weaker baseline and is not followed.

**3. `hideCredentials` already has a bearer contract**

OIDC `hideCredentials` documents and implements removal of the client `Authorization` header. Coupling Gateway cookies to that flag (default `false`) would leave the leak in place. Identity for upstream stays on `claimsToHeaders` / `accessTokenInAuthorizationHeader` / token headers.

## Fix Suggestions Not Accepted

- Gate stripping on `hideCredentials` — default `false` would keep the leak; mixes two credential classes
- Leave cookies application-visible — the application should not consume a Gateway session credential
- Add an OIDC-local Cookie parser — reuse `auth_common::strip_cookies`
- Change `hideCredentials` defaults across other auth plugins in the same change — JWT/KeyAuth already strip cookie *sources* when that flag is on; those plugins do not issue Gateway session cookies

## Re-evaluation Triggers

Re-open this decision only if:

- OIDC grows an explicit operator knob to forward the Gateway session cookie upstream
- `hideCredentials` is redesigned as a single credential-ownership flag across all auth plugins, including Gateway-issued cookies

## Reference Cases

- [review knowledge](../../SKILL.md) (D0060)
- `edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs` (`strip_gateway_owned_cookies`)
- `edgion-gateway/src/plugins/http/common/auth_common.rs` (`strip_cookies`)
- `docs/en/user-guide/edgion-plugins/openid-connect.md`

## Resolution references

- `D0060` — 2026-09-04.

## Authorization-state return URL ownership

Authorization redirect construction borrows the current request query while
formatting the state payload's owned original_url. The adapter observes the same
current URI as the former owned getter. Absent and present-empty queries both
omit the question mark; nonempty raw query bytes retain duplicates and encoding.
The final URL remains owned for encrypted cookie serialization. State, nonce,
PKCE binding and cookie-attribute behavior are unchanged.

The existing authorization redirect regression decodes the encrypted state
cookie for all four query shapes and checks original_url together with random
lengths, state/nonce equality and the PKCE digest. This verifies the local
construction contract, not a live IdP round trip or measured performance gain.

## Callback-local temporary release

handle_callback releases the raw state-cookie String once decoding returns its
owned state/error result. The decoded state still survives through redirect
target selection. Successful access-token verification is required as before,
but its otherwise unused claims Value is released before session-payload
construction and cache insertion. After cookie encoding returns an owned String
or unit error, the local session payload releases before subsequent cookie/error
response handling. Cache insertion has already completed with independent owners.

These are local ownership boundaries, not changes to state/nonce/PKCE checks,
verification, cache behavior, cookie encryption, size limits or redirect order.
Session headers, cache entries, crypto buffers and returned ciphertext remain
independent owners; drop does not establish erasure. No allocation-count, future
size, measured peak-memory or throughput improvement is claimed.

## Cookie cryptographic input release

The shared encoder releases serialized plaintext after successful encryption
before constructing Base64 output. The decoder releases decoded ciphertext
after successful decryption before deserializing JSON. Locked aes-gcm 0.10.3
uses aead 0.5.2's allocating Aead implementation, which copies input into a
separate Vec and returns that owned output; neither result borrows the input.

Only input lifetime changes. Nonce/RNG, key use, ciphertext framing, Base64,
error precedence and generic serialization/deserialization remain unchanged.
The plain Vec drops neither zeroize data nor release independent caller/crypto
owners. This input-lifetime change does not itself reduce allocation count or
establish measured peak memory or throughput improvements. The subsequent framing
change below does not introduce in-place crypto.

## Encrypted-cookie framing ownership (oidc-02)

The shared encoder now delegates its final framing stage to encode_cookie_frame.
It computes checked URL_SAFE_NO_PAD lengths, allocates one exact byte buffer,
encodes nonce and ciphertext directly into disjoint regions around one dot, and
converts that same buffer into String with safe UTF-8 validation. The two temporary
Base64 Strings and their final composition copy are removed. Zero-initialization
and final UTF-8 validation remain; there is no unsafe conversion or buffer pool.
Unrepresentable sizes and encoding/conversion failures use the existing unit-error
boundary. Representable inputs retain the same bytes. Serialization, nonce/RNG,
AES-GCM, plaintext release, decoder errors, cache order, cookie-size checks and
Set-Cookie publication remain unchanged.

Fixed-input tests compare complete output against the previous framing and a
pre-sized encode_string alternative, including binary data, empty framing-only
ciphertext, chunk boundaries and near-limit lengths. They check exact logical
output capacity and decode both regions. Actual crypto roundtrip, tamper and
persistence tests remain separate. Synthetic allocation comparisons include full
framing; their counts are not an allocator API contract. Performance evidence
must include sizing, allocation and final String conversion, and cannot establish
whole-authentication throughput, allocator RSS or secret erasure.


## Session persistence projection ownership (oidc-07)

OidcSessionCookie retains one six-field serde schema with a default String text
parameter. Decoded cookies and refresh-cache entries remain owned. Persistence uses
the same schema with borrowed str fields through for_persistence, avoiding temporary
reference and optional-token copies. A missing reference still has one local generated
String owner; existing references, including whitespace, are borrowed unchanged.
The redacted Debug implementation covers both representations without showing fields.

The trim-based access-token exclusion rule, empty-field omissions, Some(empty)
optional tokens and timestamp fields are unchanged. Cache insertion still occurs
before cipher lookup, encryption and size checks; missing cipher or oversized output
can therefore skip publication after the cache side effect. Source payloads remain
immutable. The generated owner and source borrow survive the existing cache await;
serialization produces an independent JSON buffer before encryption.

Independent frozen-schema tests compare exact JSON bytes and fixed literals, owned
deserialization and both redacted Debug forms. Real persistence tests retain encrypted
roundtrip/source checks, including cache-before-skipped-publication regressions.
Allocation comparisons include projection and JSON serialization, with a separate
synthetic generated-reference owner case. The savings concern temporary projection
fields, not cache-owned tokens, JSON, crypto or output buffers. Allocator totals and
logical copied bytes do not establish peak RSS, secret erasure or publisher-wide
zero allocation; scoped timing must not be presented as authentication throughput.
