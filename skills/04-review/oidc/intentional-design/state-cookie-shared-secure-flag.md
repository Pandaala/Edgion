---
name: state-cookie-shared-secure-flag
description: Accepted RFC 6265bis §4.1.2.2 / OAuth BCP divergence — the OIDC state cookie shares the operator-controlled session_cookie_secure flag instead of forcing Secure=true. Reject re-reports proposing an always-Secure state cookie or a separate allow-http knob.
knowledge_type: intentional-design
---

# cookie-auth-05 — OIDC state cookie shares `session_cookie_secure`

**Status:** ⚠️ accepted divergence · 2026-05-20
**RFC:** RFC 6265bis §4.1.2.2 (SHOULD); OAuth 2.0 Security BCP / RFC 9700 §4.2.4 (advisory)

## Conclusion

The `edgion_oidc_state` cookie (carrying the AES-256-GCM-encrypted `{state, original_url, code_verifier, nonce}`) gets its `Secure` attribute from the same operator-controlled `session_cookie_secure` flag as the session cookie. It is **Secure by default** (`session_cookie_secure` defaults to `true`); it only travels over HTTP when the operator explicitly opts into an HTTP-only deployment.

Suggestions of the form "force the state cookie `Secure=true` regardless of `session_cookie_secure`" or "add an `oidc_state_cookie_allow_http: bool = false` knob" are **not accepted**.

## RFC clause

> "Cookies that contain sensitive information ... SHOULD be issued with the Secure attribute." — RFC 6265bis §4.1.2.2 (advisory)

OAuth 2.0 Security BCP (RFC 9700 §4.2.4): protect intermediate artefacts (state, PKCE verifier) at least as strongly as final tokens. No hard MUST.

## Code shape today

- `edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs` — `build_cookie_header_value` appends `; Secure` iff `config.session_cookie_secure`; shared by both cookies.
- `edgion-gateway/src/plugins/http/openid_connect/callback.rs` — state cookie set on the `/authorize` redirect (max-age 300), read back on `/callback`.
- `edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs` — state payload is AEAD-encrypted (AES-256-GCM keyed by SHA256(session_secret)) before it ever hits the wire.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/openid_connect.rs` — config validation already rejects `SameSite=None` with `session_cookie_secure: false`, so the only HTTP-exposed window is `SameSite=Lax/Strict` + explicit `secure: false` (defense in depth).

## Why we diverge

1. **Forcing `Secure=true` breaks the OIDC login round-trip** — the state cookie is set on the redirect and must be returned on the callback. On an HTTP-only deployment a Secure cookie is never sent back → `/callback` reports "Missing OIDC state cookie" → login dies. This is the exact breakage class rejected in cookie-auth-04 (see [csrf/cookie-secure-configurable.md](../../csrf/regression-guards/cookie-secure-configurable.md)).
2. **Operator escape hatch** — `session_cookie_secure: false` is the documented HTTP-only path (TLS terminated upstream, mesh-internal plain HTTP). An `allow_http` knob defaulting to `false` would silently flip behavior for existing HTTP-only operators on upgrade.
3. **No real security gain from splitting the flag** — an attacker who can MITM the state cookie on HTTP can equally steal the far more valuable session cookie on the same transport. The payload is also already AEAD-encrypted, so passive capture yields ciphertext, not the code_verifier.
4. **RFC strictness is SHOULD/advisory** — project policy fixes SHOULD/MAY only when zero design conflict exists; here there is a direct conflict with the escape hatch and the login round-trip.

## Re-evaluation triggers

- OAuth/OIDC spec upgrades the state/PKCE cookie Secure requirement to a MUST.
- `PluginSession` gains a trustworthy downstream-scheme primitive that makes per-cookie Secure decisions safe without an operator flag.
- The OIDC plugin migrates `session_cookie_secure` to a different shape (keep state/session/CSRF cookie semantics consistent).

## Reference

- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a5.md → V-A5-06` (CONFIRM with nuance; encryption noted as mitigation)
- Sibling decision: cookie-auth-04 — [csrf/cookie-secure-configurable.md](../../csrf/regression-guards/cookie-secure-configurable.md)

## Resolution references

- `cookie-auth-05` — 2026-05-20 (⚠️ accepted divergence).

## Complete Cookie formatting ownership (oidc-04)

`build_cookie_header_value` constructs the complete name/value and attribute text
for authorization state, callback session establishment, logout clearing and
refresh persistence. Attribute policy was already shared; this change removes the
intermediate owned suffix and the unused session argument. Callers still select
their existing name, value, path and Max-Age. There are no prepared suffix Strings
or additional per-generation state, including for plugins that emit no cookies.

The formatter preserves exact attribute order and the existing case-insensitive
Strict/None choice with an untrimmed Lax fallback. It does not validate or escape
input, change the configured Secure/HttpOnly policy, publish headers, or move any
cookie-size check. Encryption, cache writes, failure handling and Set-Cookie append
order remain with their original owners. The final String-to-HeaderValue ownership
boundary is separate from this change.

Full-byte tests compare the previous two-stage formatter across all flag choices,
set/clear forms, lifetime boundaries, long paths/values and defensive bypass inputs.
Separate plugin generations retain independent policy. The formatter computes exact
byte capacity from borrowed fragments and the decimal Max-Age length, using checked
addition, then writes once into the preallocated String. Decimal-boundary tests and
the full-byte matrix also check exact logical output capacity. No temporary decimal
String or persistent prepared suffix is needed. If sizing is unrepresentable, the
allocation hint falls back to normal String growth without a new request error.
String writes with only standard str/u64 formatting cannot return a formatting error.

The first plain `format!` candidate was rejected: actual-helper synthetic tests
measured returned capacity growing from 59 to 116 for a clear cookie and from 1117
to 2144 for a long callback path. A returned String may remain live across an await,
so fewer intermediate owners alone did not justify that retained-memory regression.
Exact sizing addresses that growth; allocation comparisons still measure actual
calls and requested bytes without assuming an allocator-call-count contract.
Helper measurements do not establish allocator RSS, whole-authentication,
publication or network throughput improvements.
Reconsider persistent suffix preparation only with evidence about emission rates,
plugin populations, retained memory and the complete formatting lifetime.
