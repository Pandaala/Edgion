---
name: cookie-auth-05-state-cookie-shared-secure-flag
description: Accepted RFC 6265bis §4.1.2.2 / OAuth BCP divergence — the OIDC state cookie shares the operator-controlled session_cookie_secure flag instead of forcing Secure=true. Reject re-reports proposing an always-Secure state cookie or a separate allow-http knob.
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

- `plugins/http/openid_connect/session_cookie.rs:183-188` — `build_cookie_attr_suffix` appends `; Secure` iff `config.session_cookie_secure`; shared by both cookies.
- `plugins/http/openid_connect/callback.rs:169` — state cookie set on the `/authorize` redirect (max-age 300), read back on `/callback`.
- `plugins/http/openid_connect/session_cookie.rs:107-119` — state payload is AEAD-encrypted (AES-256-GCM keyed by SHA256(session_secret)) before it ever hits the wire.
- `types/resources/edgion_plugins/plugin_configs/openid_connect.rs:417-419` — config validation already rejects `SameSite=None` with `session_cookie_secure: false`, so the only HTTP-exposed window is `SameSite=Lax/Strict` + explicit `secure: false` (defense in depth).

## Why we diverge

1. **Forcing `Secure=true` breaks the OIDC login round-trip** — the state cookie is set on the redirect and must be returned on the callback. On an HTTP-only deployment a Secure cookie is never sent back → `/callback` reports "Missing OIDC state cookie" → login dies. This is the exact breakage class rejected in cookie-auth-04 (see [csrf/cookie-secure-configurable.md](../csrf/cookie-secure-configurable.md)).
2. **Operator escape hatch** — `session_cookie_secure: false` is the documented HTTP-only path (TLS terminated upstream, mesh-internal plain HTTP). An `allow_http` knob defaulting to `false` would silently flip behavior for existing HTTP-only operators on upgrade.
3. **No real security gain from splitting the flag** — an attacker who can MITM the state cookie on HTTP can equally steal the far more valuable session cookie on the same transport. The payload is also already AEAD-encrypted, so passive capture yields ciphertext, not the code_verifier.
4. **RFC strictness is SHOULD/advisory** — project policy fixes SHOULD/MAY only when zero design conflict exists; here there is a direct conflict with the escape hatch and the login round-trip.

## Re-evaluation triggers

- OAuth/OIDC spec upgrades the state/PKCE cookie Secure requirement to a MUST.
- `PluginSession` gains a trustworthy downstream-scheme primitive that makes per-cookie Secure decisions safe without an operator flag.
- The OIDC plugin migrates `session_cookie_secure` to a different shape (keep state/session/CSRF cookie semantics consistent).

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a5.md → V-A5-06` (CONFIRM with nuance; encryption noted as mitigation)
- Sibling decision: cookie-auth-04 — [csrf/cookie-secure-configurable.md](../csrf/cookie-secure-configurable.md)
- Closure: [CLOSED-FINDINGS.md → cookie-auth-05](../CLOSED-FINDINGS.md)
