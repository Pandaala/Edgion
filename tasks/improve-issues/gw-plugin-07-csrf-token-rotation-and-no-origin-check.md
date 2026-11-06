# gw-plugin-07: CSRF plugin rotates its token on every safe request and has no Origin / Fetch-Metadata defense

- **Category**: best-practice
- **Severity**: P2
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/csrf/plugin.rs:47-76` (`set_csrf_cookie`), `:88-93` (safe-method branch), `:95-147` (unsafe-method verification); `csrf/token.rs`

## Summary
The plugin is a stateless, session-unbound, signed double-submit cookie (APISIX `csrf` clone).
Two consequential gaps:
1. It mints a **new** token and appends a new `Set-Cookie` on **every** GET, HEAD, or OPTIONS,
   even when the client already holds a valid token. A SPA that reads the cookie into a header and
   then fires a mutating request while any parallel safe request is in flight races itself. The
   browser sends the newest cookie, but the header carries the older token, so `header_token !=
   cookie_token` returns a spurious 401 `csrf token mismatch`. Multiple tabs make it worse.
2. Because the token is not bound to any session, anyone who can set a cookie on the target host
   can plant a self-generated valid pair, since the token is signed but its value is not secret per
   user. Cookie tossing from a sibling subdomain or an HTTP-only sibling origin are examples. The
   plugin performs no `Origin` / `Sec-Fetch-Site` check that would stop such a cross-site request
   independently.

## Current behavior (evidence)
- `plugin.rs:88-93`: `if SAFE_METHODS.contains(&method) { self.set_csrf_cookie(..); return GoodNext; }`.
  This branch runs unconditionally, with no check for an existing valid cookie.
- `set_csrf_cookie` (`:47-76`) always generates fresh random bytes and a timestamp
  (`CsrfToken::generate`) and appends `Set-Cookie`.
- Verification (`:95-147`) is: header present → cookie present → string equality → HMAC and age.
  It does not read `Origin`, `Referer`, or `Sec-Fetch-Site`.

## Industry comparison
- **OWASP CSRF Cheat Sheet**: prefer synchronizer or *session-bound* signed double-submit tokens
  (HMAC over the session id), and use Fetch Metadata (`Sec-Fetch-Site`) and Origin verification as
  defense in depth.
- **Go 1.25 `net/http.CrossOriginProtection`** and the Rails / Django `Origin` checks reject
  cross-site unsafe requests using `Sec-Fetch-Site`, falling back to `Origin` vs `Host`. They need
  no token state and have no race.
- Django and Rails reuse an existing valid CSRF cookie instead of rotating it on every GET.
  Rotation happens on login or session change.

## Impact
- Intermittent false 403/401s for real SPAs. Operators typically "fix" this by disabling the
  plugin.
- Weaker-than-expected protection on hosts that share a cookie scope with less-trusted siblings.

## Suggested direction
1. On a safe method, mint a token only when the request has no cookie or the cookie fails
   verification or is near expiry. Otherwise leave the cookie untouched.
2. Add an Origin / Fetch-Metadata gate in front of the token check. Reject unsafe methods when
   `Sec-Fetch-Site` is `cross-site` (or `Origin` is present and not same-origin or not in an
   allow list). Optionally allow a token-less mode that relies solely on it, as Go 1.25 does.
3. Optionally allow binding the HMAC input to a session-identifying cookie or claim (for example,
   the OIDC session or a configured cookie name) for session-bound double-submit.

## Notes
- `skills/04-review/csrf/intentional-design/csrf-expires-integer-seconds-retained.md:62-65`
  acknowledges that the design is "stateless, session-unbound" and treats binding as a future
  redesign trigger. That page concerns the `expires` field type. It does not evaluate the
  per-GET rotation race or the missing Origin / Fetch-Metadata gate, which are the new evidence
  here.
- Other csrf regression guards (`set-cookie-append`, `cookie-secure-configurable`,
  `signature-input-fragments`, `execution-time-method-rewrite-boundary`) are unaffected.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/plugins/http/csrf/plugin.rs:89-92` (safe-method branch): every GET/HEAD/OPTIONS calls `set_csrf_cookie`, which always `CsrfToken::generate`s a fresh token (`:46-48`), with no check for an existing valid cookie.
- The unsafe-method path (`:96-120`) compares header/cookie and verifies HMAC/age only; there is no `Origin`, `Referer` or `Sec-Fetch-Site` read anywhere in the plugin.
- `skills/04-review/csrf/intentional-design/csrf-expires-integer-seconds-retained.md` only notes the session-unbound design as a future trigger; it does not evaluate the rotation race or Fetch-Metadata defense.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `csrf/plugin.rs` is unchanged since feature-08-30, but the original Location refs were off; updated to `set_csrf_cookie` `:47-76`, the safe-method branch `:88-93` and verification `:95-147`.
- The safe-method branch still regenerates a token on every GET/HEAD/OPTIONS (`plugin.rs:48`), and the plugin still has no Origin, Referer or Sec-Fetch-Site check.
- `csrf-expires-integer-seconds-retained.md` is unchanged.
