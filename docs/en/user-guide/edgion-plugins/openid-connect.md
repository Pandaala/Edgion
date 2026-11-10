# OpenID Connect Plugin

> **🔌 Edgion extension**
> 
> OpenidConnect is an OIDC/OAuth2 authentication plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

`OpenidConnect` is a request-phase authentication plugin that completes OIDC/OAuth2 authentication before requests reach the upstream.

## Implemented Capabilities

- Bearer Token verification (JWKS, local JWT signature verification, Introspection)
- Authorization Code Flow (with optional PKCE)
- State/Nonce verification
- Session Cookie management (AES-256-GCM encrypted)
- Stateless sessions: the encrypted cookie carries the access, refresh and ID tokens, so any
  Gateway replica or plugin generation can serve a session (no sticky sessions required)
- Session cookie chunking for large tokens
- Token refresh with singleflight concurrency control
- Logout (local cleanup, optional revoke call, optional end_session redirect)
- Claims mapping to Headers (dot-notation, injection protection, size limits)

## Configuration Parameters

### Basic Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `discovery` | String | — (required) | OIDC Discovery endpoint URL, must start with `http://` or `https://` |
| `clientId` | String | — (required) | OAuth2 Client ID |
| `clientSecretRef` | Object | none | Reference to a K8s Secret containing the client secret |
| `bearerOnly` | Boolean | `false` | Only validate Bearer Tokens, do not enable browser login flow |
| `unauthAction` | Enum | `Auth` | Behavior on unauthenticated requests. Values: `Auth` (trigger login) | `Deny` (return 401) | `Pass` (pass through) |
| `scope` | String | `"openid"` | OIDC request scope |
| `requiredScopes` | Array\<String\> | None | Required scopes; a token missing any is rejected. **Only enforced when `bearerOnly: true`** (see note below) |
| `realm` | String | `"edgion"` | Realm value in the `WWW-Authenticate` response header; [Character rules](../http-route/filters/edgion-plugins/README.md#authentication-realm) |

> **Note on `requiredScopes`:** scope enforcement only applies in bearer-only mode
> (`bearerOnly: true`). In the default session/login mode (`bearerOnly: false`, which
> includes `unauthAction: Auth`), `requiredScopes` is accepted at config time but **not**
> enforced at runtime — every authenticated user passes regardless of the token's scopes.
> To restrict access by scope, set `bearerOnly: true`.

### Authentication Flow Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `usePkce` | Boolean | `false` | Whether to enable PKCE (Proof Key for Code Exchange). **Strongly recommended `true` for production deployments** — see [Recommended Security Switches](#recommended-security-switches) |
| `useNonce` | Boolean | `false` | Whether to include nonce in authorization requests. **Strongly recommended `true` for production deployments** — see [Recommended Security Switches](#recommended-security-switches) |
| `redirectUri` | String | none | Custom callback URI, must be an absolute path or absolute URL. A path uses the request `Host` plus the listener's real TLS state; `X-Forwarded-Host` and `X-Forwarded-Proto` are both ignored. Use an absolute URL when the public origin differs, including when TLS is terminated in front of the gateway |
| `authorizationParams` | Object | none | Extra parameters appended to the authorization request (cannot contain reserved keys like `response_type`, `client_id`, etc.) |
| `revokeTokensOnLogout` | Boolean | `false` | Whether to call the Token Revocation endpoint on logout |
| `logoutPath` | String | `"/logout"` | Path that triggers logout, must start with `/` |
| `postLogoutRedirectUri` | String | none | Redirect URI after logout. A path uses the request `Host` plus the listener's real TLS state; `X-Forwarded-Host` and `X-Forwarded-Proto` are both ignored. Use an absolute URL when the public origin differs, including when TLS is terminated in front of the gateway |

### Session Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `sessionSecretRef` | Object | none | Reference to a K8s Secret containing the session encryption key (required when `unauthAction=Auth`) |
| `sessionCookieName` | String | `"edgion_oidc_session"` | Session cookie name. This cookie, its chunk cookies (`<name>_chunks`, `<name>_0` .. `<name>_15`) and `edgion_oidc_state` are Gateway-owned and are always removed from the request before proxying upstream. Other application cookies are left unchanged. |
| `sessionLifetime` | Integer | `3600` | Absolute session lifetime in seconds, fixed at login and enforced by the Gateway |
| `sessionCookieSameSite` | String | `"Lax"` | Cookie SameSite attribute. Values: `Strict` | `Lax` | `None` |
| `sessionCookieHttpOnly` | Boolean | `true` | Cookie HttpOnly attribute |
| `sessionCookieSecure` | Boolean | `true` | Cookie Secure attribute (must be `true` when `SameSite=None`) |
| `maxSessionCookieBytes` | Integer | `3800` | Maximum size in bytes of one session `Set-Cookie` string (the single cookie or each chunk cookie) |
| `maxSessionCookieChunks` | Integer | `8` | Maximum number of chunk cookies one session may use (`1`–`16`; `1` disables chunking). `maxSessionCookieBytes` × `maxSessionCookieChunks` must not exceed `32768` |
| `renewAccessTokenOnExpiry` | Boolean | `true` | Whether to auto-refresh access token on expiry |
| `accessTokenExpiresLeeway` | Integer | `0` | Seconds before expiry at which the access token is refreshed (at most half its lifetime); also how long a just-expired token may still be served while a refresh is pending |

The Gateway rejects a session at its authenticated deadline, even when a retained
cookie contains a usable refresh token or a still-valid access token.
Token refresh does not extend that deadline; refreshed cookies use only the remaining
session lifetime as `Max-Age`. Access-token expiration and session expiration are
independent. A zero or unrepresentable session lifetime cannot establish a session.

Changing `sessionLifetime` affects new logins only. Existing sessions retain their
original deadline, so increasing the value cannot revive an expired cookie and
decreasing it does not immediately terminate existing sessions. Expired sessions
follow `unauthAction`: `Deny` returns 401, `Auth` starts login again, and `Pass`
continues without authenticating the session. An existing IdP login may complete
the new login without prompting for a password.

Gateway clocks must be synchronized: future issue times are rejected, and token
clock-skew/leeway settings do not extend session deadlines. Authentication is
checked again after asynchronous verification/refresh work before accepting the
session. This does not disconnect streams already admitted. Logout can still clear
an expired session and use its tokens for the configured logout/revocation flow.
Bearer authentication is independent of the session deadline.

Cookies issued before server-side deadlines were introduced lack the required
deadline and require a new login after upgrading. Complete the Gateway rollout
before relying on server-side enforcement; older binaries do not enforce it.

### Stateless sessions and cookie chunking

The session cookie is the whole session: it holds the access token, the refresh and ID
tokens (when issued), the refresh horizon and the absolute deadline, sealed with
AES-256-GCM. The Gateway keeps no per-replica session state, so a request can land on any
replica, after a restart, or after an EdgionPlugins update, and still authenticate without
refreshing tokens. Token verification itself is unchanged: a cold replica still fetches
discovery/JWKS, and introspection results are cached per replica. A `sessionSecretRef`
change still invalidates sessions.

When the sealed value does not fit in one cookie of `maxSessionCookieBytes`, it is split
into `<name>_0` .. `<name>_{N-1}` plus a `<name>_chunks` cookie holding `N` (the naming
used by Envoy's OAuth2 filter). Every write expires the cookies of the other shape, and
logout expires the single cookie and all chunk slots. A request whose chunk set is
incomplete or invalid, or that carries both a chunk set and the single cookie, is treated
as unauthenticated (it never falls back to the other shape). If a session cannot fit in
`maxSessionCookieChunks` cookies, login fails with `Session cookie exceeds configured size
limit`; if a refreshed session cannot fit, the Gateway logs `session cookie too large` and
ends the session (the current request still proceeds) instead of keeping a cookie whose
refresh token may already be rotated.

Refresh requires `renewAccessTokenOnExpiry: true` and a refresh token. It runs when the
access token is rejected as expired, when the cookie carries no access token, or once the
refresh horizon is reached. The horizon is `expires_in` minus the larger of
`clockSkewSeconds` and `accessTokenExpiresLeeway`, never more than half the token lifetime;
300 s is assumed when the identity provider omits `expires_in` or reports `0`. Opaque tokens verified by
introspection rely on the horizon. Concurrent requests on one replica share one refresh;
separate replicas refreshing the same expired session may each call the token endpoint,
which identity providers with strict refresh-token reuse detection can treat as reuse.

Operational and security notes:

- Prefer a `__Host-` prefixed `sessionCookieName` (for example `__Host-edgion_oidc_session`,
  with the default `sessionCookieSecure: true`). The chunk names inherit the prefix, so
  browsers refuse to let a non-secure origin or a sibling subdomain plant chunk cookies.
- Logout expires the cookies in the browser only. A copy of the cookie taken before logout
  carries the refresh token, so with the defaults it keeps authenticating (refreshing as
  needed) until the absolute session deadline (`sessionLifetime`). Enable
  `revokeTokensOnLogout` so the refresh token is revoked at logout; with local JWKS
  verification the current access token still works until it expires, so also use
  introspection-based verification when revoked tokens must stop working immediately.
- Cookies issued before stateless sessions carry no access token: they authenticate only
  through a refresh (refresh token present and `renewAccessTokenOnExpiry: true`); otherwise
  the user logs in again.
- The default budget (3800 × 8 bytes) keeps the Cookie header well below the downstream
  HTTP/2 header-list limit (64 KiB). Front proxies and CDNs may need larger request and
  response header buffers (for example nginx `large_client_header_buffers` and
  `proxy_buffer_size`) when tokens are large.
- Application cookies named `<sessionCookieName>_chunks` or `<sessionCookieName>_0` ..
  `_15` are treated as Gateway-owned and stripped; choose a `sessionCookieName` that does
  not collide with them. Renaming `sessionCookieName` orphans existing cookies until they
  expire; the old names are no longer stripped, so those sealed cookies reach the upstream
  until then.
- Lowering `maxSessionCookieBytes` can invalidate existing chunked sessions whose
  reassembled size exceeds 16 × the new value.

### Token Verification Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `verificationMode` | Enum | `Auto` | Verification mode. Values: `Auto` | `JwksOnly` | `IntrospectionOnly` |
| `tokenSigningAlg` | String | none | Specify token signing algorithm (cannot be `none`) |
| `allowedSigningAlgs` | Array\<String\> | none | Allowed signing algorithms list (cannot contain `none`) |
| `useJwks` | Boolean | `true` | Whether to use JWKS for local verification |
| `verify` | Boolean | `true` | Verify the OIDC Provider's TLS certificate. When `false`, the gateway skips peer certificate validation (operator responsibility; a WARN is logged at startup). For private-CA IdPs, pair with `validation.caCertificateRefs` (list of Secret/ConfigMap refs each with a `ca.crt` key) or `validation.wellKnownCACertificates: System`. `validation.subjectAltNames` adds SAN pinning and forces full certificate verification. For mutual TLS to the IdP, pair with `clientCertificateRef` (Secret with `tls.crt`/`tls.key` keys). Unresolved, partial, or malformed declared TLS material rejects OIDC plugin construction and seals the affected route fail-closed; it never falls back to a client that omits the declared policy. Inline CA (`caCert`) is no longer supported. Note: `clientSecretRef` in the Basic Parameters table above is the OAuth2 client secret, separate from TLS configuration. |
| `issuers` | Array\<String\> | none | Allowed issuer list |
| `audiences` | Array\<String\> | none | Allowed audience list |
| `clockSkewSeconds` | Integer | `120` | Clock skew tolerance (seconds) |
| `jwksCacheTtl` | Integer | `300` | JWKS cache TTL (seconds). Range: 1 to 31536000 (one year) |
| `jwksMinRefreshInterval` | Integer | `10` | Minimum JWKS refresh interval (seconds). Range: 1 to 31536000 (one year) |
| `introspectionEndpoint` | String | none | Custom introspection endpoint URL |
| `introspectionEndpointAuthMethod` | Enum | `client_secret_basic` | Introspection endpoint auth method. Values: `client_secret_basic` | `client_secret_post` |
| `introspectionCacheTtl` | Integer | `0` | Introspection result cache TTL (seconds), `0` disables |
| `tokenEndpointAuthMethod` | Enum | `client_secret_basic` | Token endpoint auth method. Values: `client_secret_basic` | `client_secret_post` |

A failed discovery fetch is remembered for 5 seconds so concurrent or immediately following requests reuse that failure instead of contacting the identity provider again. The next call after that window retries; a successful document replaces the failure and is cached for the life of the plugin generation. This interval is not configurable.

### Cache Selection and Limits

Only positive introspection results use TinyUFO/S3-FIFO:

```yaml
introspection:
  tinyUfoTtlCache:
    maxEntries: 2048
    maxLogicalBytes: 8MiB
```

The section inherits the `tiny_ufo_ttl_cache` Gateway defaults. Set both override
fields together, or omit both to inherit the process defaults. The byte field is
the cache instance's total logical record-weight budget, not an exact RSS limit.
There is no access-token cache: the session cookie carries the access token. The
short-lived (5 s) refresh-result cache that lets concurrent requests share one
refresh uses the Gateway `lru_cache` defaults. Discovery and JWKS retain their
dedicated snapshot providers; the refresh singleflight registry is separate.

### Header Forwarding Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `setAccessTokenHeader` | Boolean | `false` | Whether to pass the access token to upstream |
| `setIdTokenHeader` | Boolean | `false` | Whether to pass the ID token to upstream |
| `setUserinfoHeader` | Boolean | `false` | Whether to pass userinfo to upstream |
| `accessTokenInAuthorizationHeader` | Boolean | `false` | Whether to set the access token in the upstream `Authorization: Bearer` header |
| `claimsToHeaders` | Object | none | Claims-to-header mapping (supports dot-notation, e.g., `user.name`). Destination names are case-insensitive and must be unique within this plugin instance |
| `storeClaimsInCtx` | Boolean | `false` | Whether to store claims in the ctx variable `oidc_claims` (read by downstream plugins via `get_ctx_var`; excluded from the access log so claims are not persisted) |
| `maxHeaderValueBytes` | Integer | `4096` | Maximum size per mapped header value in bytes |
| `maxTotalHeaderBytes` | Integer | `16384` | Maximum total size for all mapped headers in bytes |

After successful authentication, all configured `claimsToHeaders` destinations are cleared before upstream headers are produced. A missing, empty, unsupported, unsafe, or oversized claim leaves its destination absent. Cleanup also runs when fixed token outputs exhaust the total header budget; cleanup failure stops forwarding with a `401` (see below). Skipped or disabled plugin paths are outside this sanitation contract.

`claimsToHeaders` cannot target `Host` or a framing/hop-by-hop header (`Content-Length`, `Transfer-Encoding`, `TE`, `Trailer`, `Expect`, `Connection`, `Keep-Alive`, `Proxy-Connection`, `Upgrade`) (case-insensitive). Configuration validation rejects them: HTTP/2 `:authority` can recreate `Host` during protocol conversion, and destinations are cleared on every request, which would change request framing. Use a dedicated identity header such as `X-Tenant`.

Claim-header destinations are canonicalized and ordered before request processing. Case variants such as `X-User` and `x-user` therefore conflict within one OpenID Connect plugin configuration and are rejected. Separate plugins retain execution-order, last-write-wins behavior.

### Security Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `hideCredentials` | Boolean | `false` | Whether to remove the original client `Authorization` header after successful bearer auth. This flag does not control session or state cookies; those Gateway-owned cookies are always stripped before proxying. |
| `authFailureDelayMs` | Integer | `0` | Authentication failure delay (milliseconds), mitigates brute-force attacks |

### Network Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `timeout` | Integer | `3` | OIDC Provider request timeout (seconds) |

Token exchange, token refresh, and introspection responses have a fixed **1 MiB
(1,048,576 bytes)** body limit, including JSON whitespace. The limit is enforced
while reading, including chunked responses. Oversized, unreadable, or invalid JSON
responses fail the exchange; they are never truncated and used as credentials.
When returned to the client as an upstream error, these failures use **502 Bad
Gateway** without provider details. Existing session-refresh fallback and
`unauthAction` behavior still apply. In Auto mode, failed introspection after an
invalid JWT retains the original 401 rejection. This limit is not configurable.

---

### Configured projection headers

When this plugin executes, all configured projection destinations are removed before
bounded claim/payload projection. Missing, empty, unsupported, unsafe, or oversized
values leave their destination absent, including destinations after aggregate-budget
exhaustion. Removal or projection write failure is rejected with `401 Unauthorized -
Authentication required` after `authFailureDelayMs`, records the diagnostic
`AuthHeaderMutationRejected`, and does not forward the request.
This also applies to unauthenticated `Pass`. The enabled fixed outputs (`X-Access-Token`,
`X-ID-Token`, `X-Userinfo`, and `Authorization` in session-cookie mode with
`accessTokenInAuthorizationHeader`) are removed on every continuation before writing, so an
oversized value, or one that is not valid HTTP header text (for example a control byte), is
skipped and leaves the header absent instead of a client-supplied copy — a skipped value is not
a failure and does not deny; in bearer mode the verified client `Authorization` is kept. A
`hideCredentials` removal failure is rejected the same way.
`AuthHeaderMutationRejected` is an access-log diagnostic (an `error` entry in the `diagnostics`
array), not a metric or an alert condition by itself; the code is shared with other auth plugins,
where a client can trigger it (a hidden query credential sent to a gRPC-JSON-transcoded route,
whose query is sealed). Alert on its rate with log-based alerting.
Unconfigured names are unaffected. Disabled or condition-skipped plugins do not run
cleanup; these paths provide no authenticated-header guarantee.

## Recommended Security Switches

`usePkce` and `useNonce` both default to `false`, matching the defaults of peer gateways (Kong, APISIX, Envoy — the OIDC spec defines both as OPTIONAL for authorization code flow). **However, for production deployments both should be set to `true`**. They provide defense-in-depth for the authorization code flow, cost is negligible, and all mainstream IdPs (Keycloak, Auth0, Okta, Google, Azure AD) support them natively.

### `useNonce: true` (Protect Against ID Token Replay and Cross-Session Injection)

- **Threat mitigated**: An attacker obtains a legitimate ID Token from their own OIDC session and attempts to inject it into a victim's callback flow; or replay the same token multiple times
- **How it works**: Edgion includes a random `nonce` in the `/authorize` request and stores it in a signed state cookie; at callback time, the `nonce` claim in the ID Token must match
- **Why the threat is partially mitigated in Edgion's code path**: Authorization code flow + server-side token exchange means the ID Token never passes through the browser; `state` validation blocks classic CSRF. Nonce adds value as defense-in-depth (against IdP-side replay, cross-client token mixing, etc.)
- **Cost**: The IdP must echo `nonce` into the ID Token's `nonce` claim. Mainstream IdPs do this automatically; non-standard / legacy IdPs may not — validate in a staging environment first

### `usePkce: true` (Protect Against Authorization Code Interception)

- **Threat mitigated**: In public-client scenarios (SPA / native apps), the authorization code is intercepted on the `redirect_uri` by a malicious third-party app and used to exchange for tokens
- **How it works**: Edgion includes `code_challenge` (SHA-256(code_verifier)) in the `/authorize` request; at token exchange, it sends `code_verifier`; the IdP verifies they bind
- **Applicability**: Even though Edgion is a "confidential client" (has client_secret), enabling PKCE is an OAuth 2.1 best practice and provides transparent extra binding for IdPs
- **Cost**: The IdP must support PKCE (RFC 7636). Mainstream IdPs support it by default

### Recommended Minimal Production Configuration

```yaml
config:
  usePkce: true              # required
  useNonce: true             # required
  sessionCookieSecure: true      # already default true, keep
  sessionCookieSameSite: "Lax"   # already default Lax, adjust per cross-site needs
  verify: true                   # already default true, never disable
  verificationMode: Auto
```

### When You May Leave Them Off

- **Closed internal environment + bearer-only mode** (no browser interaction, no code flow): `useNonce` / `usePkce` have no effect — leaving `false` is fine
- **IdP does not support nonce / PKCE and cannot be replaced** (rare): keep `false`, but compensate via other controls (stricter `sessionCookieSameSite=Strict`, IdP-side referrer checks, etc.)

---

## Quick Start

### Minimal Configuration Example (Bearer Only)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: oidc-api
  namespace: production
spec:
  requestPlugins:
    - type: OpenidConnect
      config:
        discovery: "https://idp.example.com/.well-known/openid-configuration"
        clientId: "my-api"
        bearerOnly: true
        verificationMode: JwksOnly
        unauthAction: Deny
```

## Web Login Example (Code Flow + PKCE)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: oidc-web
  namespace: production
spec:
  requestPlugins:
    - type: OpenidConnect
      config:
        discovery: "https://idp.example.com/.well-known/openid-configuration"
        clientId: "web-app"
        clientSecretRef:
          name: oidc-client-secret
        bearerOnly: false
        unauthAction: Auth
        usePkce: true
        useNonce: true
        sessionSecretRef:
          name: oidc-session-secret
```

## Secret Constraints

- `clientSecretRef`: reads from `clientSecret` / `client_secret` / `secret`
- `sessionSecretRef`: reads from `sessionSecret` / `session_secret` / `secret`
- `sessionSecret` must be at least 32 bytes (used for AES-256-GCM)
- When the configured mode uses a secret, a referenced Secret without a usable
  value is reported on the resource status: no accepted key → `ResolvedRefs=False`
  with reason `MissingKeys`; an empty or non-UTF-8 value, or a session secret
  shorter than 32 bytes → `DecodeError`. The client secret is checked unless the
  plugin is bearer-only with `verificationMode: JwksOnly` and no
  `revokeTokensOnLogout`; the session secret whenever `bearerOnly` is false. A
  reference the mode cannot use is not checked for usable content, but missing
  Secrets and denied references are still reported.

The Controller normally resolves both references and ships their values with a
plugin generation. The Gateway consumes those resolved values when it constructs
that generation. If a resolved value is absent or empty, the Gateway performs its
legacy Secret-store fallback once during construction; requests never reload a
Secret. The session secret is immediately derived into the generation's
AES-256-GCM cipher, and the raw session-secret string is not retained in the
runtime plugin configuration. The OAuth client secret is retained as one opaque
generation value for IdP client authentication.

A Secret update becomes effective when the replacement plugin generation is
published. Existing requests remain pinned to their current generation. Rotating
`sessionSecretRef` invalidates cookies encrypted by the previous generation, so
plan for users to authenticate again after a session-secret rotation.

When a Secret is required by the active mode and request path, missing, empty, or
non-UTF-8 material—or a session secret shorter than 32 bytes—fails with the same
detail-free `401 Unauthorized` response and configured Bearer challenge, after
`authFailureDelayMs`. The basic `bearerOnly: true` + `verificationMode: JwksOnly`
path does not require an OAuth client secret; features that call a
client-authenticated IdP endpoint do. The access log records only a fixed failure
category and does not include Secret identity or content.

IdP transport, response-decoding, and request-time TLS failures are independent
dependency failures and are not governed by this credential-401 contract.
When these failures terminate authentication, the HTTP response is the fixed
`502 Bad Gateway`, without `WWW-Authenticate` or `authFailureDelayMs`. Responses
and access logs omit IdP URLs, provider HTTP statuses, response bodies, and parser
error text. Use the static access-log failure category to identify the operation
and cause, then inspect the IdP for details. Declared TLS CA and mTLS material is
consumed by the generation's canonical outbound client;
unresolved or malformed declared material follows the construction-time
fail-closed behavior described above, never these authentication-credential
fallback rules.

This error-response policy preserves authentication fallbacks: Auto mode keeps
the original 401 when introspection fails after an invalid JWT format; session
refresh failures still follow existing-token verification and `unauthAction`.
JWKS can still serve a stale cached set, userinfo can fall back to verified
claims, and failed revocation does not prevent local logout. Insufficient scope
on a Bearer request still returns 403 with the `insufficient_scope` challenge.
On gRPC routes, the Gateway's standard response adapter maps dependency 502
rejections to `INTERNAL` with the fixed `grpc-message` `502 Bad Gateway`.

## Outbound egress

Every OpenID Connect outbound call — configured discovery and discovered JWKS, token, introspection, userinfo, and revocation endpoints — connects only to addresses allowed by the gateway outbound egress policy. Hostname destinations are classified by `EgressResolver` at dial time. IP-literal destinations are classified before reqwest connects, because reqwest skips custom DNS for literals. Both the default shared client and the SAN-pinning client ignore `HTTP_PROXY` / `HTTPS_PROXY` and do not follow redirects. Loopback, link-local, and unspecified destinations are always rejected. RFC1918 private destinations follow `outbound_egress_block_private` / `EDGION_OUTBOUND_BLOCK_PRIVATE` (default `false`), so a typical in-cluster identity provider remains reachable unless that switch is enabled. Denied destinations fail closed without including endpoints or resolved addresses in public bodies or PluginLog tags. JWKS may still serve a previously cached set when a later refresh fails; that stale-serve path is unchanged. TLS SNI, certificate verification, custom CA, and SAN pinning still use the original hostname. See [Egress policy (SSRF protection)](../../ops-guide/infrastructure/connectors.md#egress-policy-ssrf-protection).

## Security Defaults

- Tokens are not passed to upstream by default
- Header injection protection: rejects `\r`, `\n`, `\0`
- Header size limits:
  - `maxHeaderValueBytes` (default `4096`)
  - `maxTotalHeaderBytes` (default `16384`)
- Session cookie size limits: `maxSessionCookieBytes` (default `3800`) per cookie and
  `maxSessionCookieChunks` (default `8`) cookies per session

## Common Troubleshooting

- `401 Unauthorized - Missing bearer token`
  - `bearerOnly=true` and the request does not carry `Authorization: Bearer ...`
- IdP dependency failure
  - Dependency failures that terminate the request return the fixed `502 Bad Gateway`; inspect the access-log operation and static failure category, and the existing outbound TLS SAN mismatch metric for SAN verification failures
- `Session cookie exceeds configured size limit`
  - The sealed session (access, refresh and ID tokens) needs more than `maxSessionCookieChunks` cookies; reduce token size at the identity provider, or raise `maxSessionCookieChunks` within the 32768-byte budget

## Related Documentation

- [JwtAuth Plugin](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT token authentication
- [ForwardAuth Plugin](forward-auth.md) - External authentication plugin
- [JWE Decrypt Plugin](jwe-decrypt.md) - JWE decryption authentication

### Request Cookie cleanup failures

Every path that continues upstream removes the Gateway session cookie, its chunk
cookies and the state cookie atomically, including unauthenticated Pass. A non-UTF-8 Cookie field makes cleanup
fail with HTTP 400; an internal update failure returns HTTP 500. The original
unsanitized request is not forwarded. Application cookie names and values survive
successful cleanup, though separators and field grouping may be normalized.
No matching Gateway cookie leaves the raw fields unchanged.
