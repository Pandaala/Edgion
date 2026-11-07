---
name: stateless-session-cookie
description: Use when reviewing where the OpenID Connect plugin keeps session state, how large session cookies are chunked, or when an OIDC access-token refresh may run.
status: fixed
finding_id: gw-plugin-03
closed: 2026-11-07
knowledge_type: regression-guards
---

# OIDC sessions are stateless sealed cookies

## Conclusion and scope

The `OpenidConnect` Gateway plugin keeps no per-replica or per-generation session state.
The AES-256-GCM sealed `OidcSessionCookie` is the whole session: access token, optional
refresh and ID tokens, the refresh horizon `expires_at`, `created_at` and the absolute
`session_expires_at`. Any replica, a restarted Gateway or a new plugin generation that holds
the same session secret authenticates the cookie without a token refresh. Token
verification is unchanged (cold JWKS/discovery fetch; per-replica introspection cache).

Fix suggestions that reintroduce an in-memory access-token cache, a `session_ref`
indirection or any other node-local source of truth for a session are **not accepted**.
A shared store is a separate design (see re-evaluation).

## Rationale and evidence

Root cause (gw-plugin-03): the callback cached the access token in a per-instance
`ShardedLruCache` keyed by a random `session_ref` and cleared it from the cookie, and every
rewrite blanked it again. A request on another replica, after a restart or after an
EdgionPlugins update missed that cache, then either refreshed (several replicas could replay
one rotating refresh token, which identity providers revoke as reuse) or logged the user out.
The cache TTL was also the only refresh horizon for opaque/introspection tokens, which fail
as `InactiveToken`, not `Expired`.

Current behavior, `edgion-gateway/src/plugins/http/openid_connect/`:

- `callback.rs` (`new_session_payload`) seals `token_response.access_token`;
  `refresh_horizon` (`token_exchange.rs`) stores `now + ttl - min(max(clockSkewSeconds, accessTokenExpiresLeeway), ttl / 2)`
  with `ttl = expires_in` or 300 s. Folding the leeway into the stored horizon (not adding it
  at check time) is what prevents a refresh on every request for short-lived tokens.
- `token_exchange.rs`: `should_preemptive_refresh(&payload)` refreshes when the knob is on,
  a refresh token exists and the token is trim-empty or `now >= expires_at`;
  `apply_refresh_response` rejects a trim-empty token (`InvalidDocument`) so a token-less
  session is never persisted. The former unconditional cache-miss refresh branch
  (which ignored `renewAccessTokenOnExpiry`) is gone.
- `session_cookie.rs`: Envoy-style chunking. A sealed value that does not fit
  `maxSessionCookieBytes` is written as `<name>_0..N-1` plus `<name>_chunks=N`
  (`2 <= N <= maxSessionCookieChunks <= 16`). Capacity uses `checked_sub` with the
  `_chunks` suffix as worst-case overhead, so every chunk and the count cookie fit the limit
  and a tiny limit errors instead of wrapping. Writes are last-writer-wins: a single write
  expires the count and prior chunks; a chunked write expires the single cookie and stale
  higher indexes. The reader accepts a count of 2..=16 with every chunk present and a total
  of at most 16 x `maxSessionCookieBytes` and no single cookie alongside; otherwise it fails
  closed (`MalformedCookie`, PluginLog `session cookie malformed`) and never falls back. The
  derived names are outside the browser's Secure-cookie overwrite protection for the single
  name, so a fallback would let a planted count/chunk cookie choose the session (cookie
  tossing / login CSRF). Logout expires all 18 session names; upstream stripping
  removes all 18 plus the state cookie in one plan.
- A refresh rewrite that cannot fit pushes `session cookie too large` and expires the session
  instead of keeping a cookie whose refresh token may already be rotated.
- `edgion-resources` validates `maxSessionCookieChunks` in 1..=16 and
  `maxSessionCookieBytes x maxSessionCookieChunks <= 32768` (checked multiply), keeping the
  Cookie header well below the downstream HTTP/2 64 KiB header-list limit.

Envoy's oauth2 filter is the reference model (cookie-held tokens, inline refresh, chunk
naming), but Edgion deliberately keeps one sealed AEAD payload, always-on chunking and
upstream stripping without re-insertion.

## Alternatives and limits

- Rejected: keep the cache as an accelerator — every request decrypts the cookie anyway.
- Rejected: Redis-backed session store — adds an auth runtime dependency; only worth it with
  server-side revocation (re-evaluation trigger).
- Accepted limit: two replicas refreshing the same expired session may both call the token
  endpoint (Envoy has the same behavior); this now happens only once a token reaches its
  refresh horizon (expiry minus the clamped margin), not on every replica switch. The per-process single-flight and 5 s refresh-result cache remain.
- Accepted limit: fail-closed shape selection turns a planted `Domain=` count/chunk cookie
  (from a sibling subdomain, when the name lacks `__Host-`) into a login lockout until that
  cookie expires; a `__Host-` session cookie name removes it. Empty-valued shape cookies count
  as absent so jars that keep expired cookies as `name=` are not locked out.
- Accepted limit: logout with an ambiguous or malformed session still expires every cookie
  locally, but cannot decode tokens for IdP revocation or `id_token_hint`.
- Accepted limit: Set-Cookie lines apply independently (RFC 6265), so a request racing a
  half-applied rewrite can see a mixed set; it fails closed and re-authenticates.
- Accepted limit: logout only expires browser cookies (Envoy behaves the same). A cookie
  copied before logout carries the refresh token, so with the defaults it keeps
  authenticating (refreshing) until `session_expires_at`. `revokeTokensOnLogout` stops the
  refresh; introspection-based verification also stops the current access token.
- Accepted limit: mixed-version rollouts and `sessionCookieName` renames can leave chunk
  cookies unstripped by old binaries or orphaned until Max-Age (pre-release, no compat layer).

## Verification and re-evaluation

Fresh verification (2026-11-07): `cargo test -p edgion-gateway --lib openid_connect`
(199 passed; the 16 ignored local-IdP tests also pass with `-- --ignored`),
`cargo test -p edgion-resources --lib` (all passed), and
`target/debug/test_client -g --dynamic-scenario OidcSessionExpiry` passed on a shared local
topology loaded with `base,HTTPRoute/Basic,EdgionPlugins/base` (run with
`--skip-residue-check --keep-alive`; the residue wipe otherwise removes the base Gateway).

Unit tests (`edgion-gateway/tests/unit/plugins/http/openid_connect/plugin_test.rs` and
`plugin_test/session_expiry_tests.rs`): exact-JSON schema oracle with legacy `session_ref`
decode; rewritten cookie carries the token; a freshly built plugin instance authenticates the
cookie with no token endpoint; refresh-horizon clamp; preemptive table incl. knob-gated
token-less case; chunk split/round-trip/limits, stale-index expiry, tiny-limit errors,
malformed/ambiguous sets fail closed, 19-name strip, custom cookie names, a line-length
property over limits; rewrite overflow ends the session; callback payload and cookie list;
refresh responses without a token are rejected; a refresh from a chunked cookie expires the
previous chunks. Resources tests cover the
new limits. The `OidcSessionExpiry` dynamic scenario adds a chunked cookie (200, Gateway
cookies stripped upstream) and a malformed chunk set (401).

Re-evaluate if server-side revocation or a shared session store is required, if sliding
sessions are introduced, or if the downstream header limits change.

## Resolution references

- gw-plugin-03 — fixed 2026-11-07. Design: `docs/superpowers/specs/2026-11-07-oidc-stateless-session-cookie-design.md`.
- Supersedes `../../cpu-memory/superseded/oidc-access-token-cache-ttl-only.md` (gw-plugin-oidc-04).
