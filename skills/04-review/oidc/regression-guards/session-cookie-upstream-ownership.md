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

`edgion_oidc_session` (or `sessionCookieName`) and `edgion_oidc_state` are Gateway credentials. Every request path that continues upstream strips both names through `auth_common::strip_cookie` before proxying. `hideCredentials` remains bearer-`Authorization` only.

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
- Add an OIDC-local Cookie parser — reuse `auth_common::strip_cookie`
- Change `hideCredentials` defaults across other auth plugins in the same change — JWT/KeyAuth already strip cookie *sources* when that flag is on; those plugins do not issue Gateway session cookies

## Re-evaluation Triggers

Re-open this decision only if:

- OIDC grows an explicit operator knob to forward the Gateway session cookie upstream
- `hideCredentials` is redesigned as a single credential-ownership flag across all auth plugins, including Gateway-issued cookies

## Reference Cases

- [review knowledge](../../SKILL.md) (D0060)
- `edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs` (`strip_gateway_owned_cookies`)
- `edgion-gateway/src/plugins/http/common/auth_common.rs` (`strip_cookie`)
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
