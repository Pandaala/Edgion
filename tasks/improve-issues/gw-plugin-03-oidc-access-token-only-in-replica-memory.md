# gw-plugin-03: OIDC session keeps the access token only in per-replica, per-generation memory

- **Category**: design-error
- **Severity**: P1
- **Scope**: large-direction
- **Location**: `edgion-gateway/src/plugins/http/openid_connect/plugin.rs:218-228` (`for_persistence`), `:310`/`:504` (`access_token_cache`); `callback.rs:309-315`; `token_exchange.rs:19-47,56-61,80-117`; `request_filter_impl.rs:63-117`

## Summary
The encrypted session cookie never contains the access token. At login (`callback.rs:309-315`)
the token goes into an in-process `ShardedLruCache` owned by the `OpenidConnect` plugin instance,
keyed by a random `session_ref`. `access_token` is cleared before the cookie is sealed. On every
later cookie write, `for_persistence` blanks it again (`cache_token = !access_token.is_empty()`, so
it is always blanked when a token exists). This silently makes the plugin stateful per replica and
per plugin generation, which conflicts with how a gateway Deployment runs: several replicas behind
a non-sticky Service, rolling restarts, and hot-reloaded EdgionPlugins.

## Current behavior (evidence)
- `request_filter_impl.rs:70` calls `resolve_access_token_from_session`. The cookie's
  `access_token` is empty, so it falls through to `get_cached_access_token(session_ref)`
  (`token_exchange.rs:40-47`), which is local memory only.
- On a cache miss with a refresh token, `:75-95` performs a token refresh against the IdP. Without
  a refresh token, `token` stays empty, `:117` logs `session no token`, and `unauthAction` applies,
  which means a redirect to login or a 401.
- The cache lives in the plugin struct (`plugin.rs:310`, built at `:504`). A new plugin generation
  starts it empty. The user doc confirms that discovery, too, is cached "for the life of the plugin
  generation".
- Refresh single-flight (`refresh_flight_registry.rs`, `refresh_singleflight_results`) is also
  per process.

Consequences:
1. **Multi-replica**: every request that lands on a replica that did not perform the login or last
   refresh triggers a refresh. A browser page that fans out N parallel asset or XHR requests across
   R replicas can issue up to R concurrent refreshes with the **same** refresh token. With
   refresh-token rotation and reuse detection (the Keycloak, Okta, Auth0, and Entra defaults or
   recommendations), the IdP treats the second use as token theft and revokes the whole token
   family, logging the user out.
2. **No refresh token issued** (no `offline_access`, or an IdP or client policy that doesn't issue
   one): every replica switch, gateway restart, rollout, or *any* edit of the EdgionPlugins
   resource logs every user out.
3. **Refresh storms**: a rollout or config change empties all caches at once, and every active
   session hits the IdP token endpoint simultaneously.

## Industry comparison
- **Envoy `oauth2` filter**: stores `BearerToken`, `IdToken`, and `RefreshToken` in (HMAC-protected)
  cookies, so any replica can serve the session statelessly.
- **oauth2-proxy**: either a cookie session store with automatic cookie *splitting*
  (`_oauth2_proxy_0/_1…`) or an explicit shared Redis session store.
- **APISIX `openid-connect`** (lua-resty-session) and **Kong OIDC** use cookie storage with chunking
  by default, or an explicit shared backend (Redis, memcache). Neither defaults to node-local memory.

## Impact
Users get random logouts and repeated IdP round-trips in any HA deployment. With rotation-enabled
IdPs, sessions can be revoked outright. Every config reload logs users out en masse when refresh
tokens are absent. None of this shows in single-replica tests.

## Suggested direction
Options:
- **A (recommended)**: Store the access token in the AEAD-sealed session cookie, as Envoy does.
  Implement cookie chunking (`name.0`, `name.1`, …, bounded count, and a total cap replacing
  `maxSessionCookieBytes`) so the access, ID, and refresh tokens fit. Keep the in-memory cache only
  as an optional decode or verify accelerator, never as the source of truth.
- **B**: An explicit, opt-in shared session store through LinkSys `RedisLinkClient` (CLAUDE.md rule
  5), keyed by `session_ref`, with server-side revocation as a bonus. This adds a runtime
  dependency, so it needs an auth fail-close policy.
- In either case, cross-replica refresh needs a reuse-tolerant strategy. Either accept a short
  grace window by serving the just-expired token within `accessTokenExpiresLeeway` while another
  replica refreshes, or use B's store as a distributed single-flight.

Option A is simplest and matches the stateless-gateway model. B is for deployments that need
revocation.

## Notes
- `docs/en/user-guide/edgion-plugins/openid-connect.md:15` advertises "In-session access token
  memory cache (cookie does not persist the access token)". That documents the mechanism, but the
  multi-replica and reload consequences are documented nowhere.
- Checked `skills/04-review/oidc/` (intentional-design: nonce-pkce-defaults,
  session-secret-resolve-per-request, state-cookie-shared-secure-flag, …; regression-guards:
  session-cookie-upstream-ownership, …). None evaluates the replica or generation statefulness of
  the access-token cache.
- No related task under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- `edgion-gateway/src/plugins/http/openid_connect/callback.rs:303-309`: after login the access token is put into `access_token_cache` and then cleared from the cookie payload; `plugin.rs:219-228` `for_persistence` blanks it on every later write when cached.
- `token_exchange.rs:40-61`: `resolve_access_token_from_session` falls back to the in-process `access_token_cache` (`ShardedLruCache` owned by the plugin instance), so another replica or a new plugin generation always misses.
- No `skills/04-review/oidc/*` or architecture page evaluates replica/generation statefulness; the user doc only states the mechanism.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- OIDC changed (absolute session deadline OIDC-01): `session_expires_at` was added to `OidcSessionCookie` (`plugin.rs:209`) and `remaining_lifetime` checks were added (`session_cookie.rs:9-18`, `request_filter_impl.rs:64-68`, `token_exchange.rs:85-110,343-400`). This does not change token storage.
- The access token is still cached in the per-instance `access_token_cache` (`plugin.rs:310,504`) and cleared from the cookie (`callback.rs:309-315`). `for_persistence` (`plugin.rs:218-228`) still blanks it.
- The updated `oidc/regression-guards/session-cookie-upstream-ownership.md` explicitly states the change "introduces neither a session store". Line references in this finding were updated.
