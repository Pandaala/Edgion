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
- In-session access token memory cache (cookie does not persist the access token)
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
| `sessionCookieName` | String | `"edgion_oidc_session"` | Session cookie name. This cookie and `edgion_oidc_state` are Gateway-owned and are always removed from the request before proxying upstream. Application cookies are left unchanged. |
| `sessionLifetime` | Integer | `3600` | Session lifetime (seconds) |
| `sessionCookieSameSite` | String | `"Lax"` | Cookie SameSite attribute. Values: `Strict` | `Lax` | `None` |
| `sessionCookieHttpOnly` | Boolean | `true` | Cookie HttpOnly attribute |
| `sessionCookieSecure` | Boolean | `true` | Cookie Secure attribute (must be `true` when `SameSite=None`) |
| `maxSessionCookieBytes` | Integer | `3800` | Maximum session cookie size in bytes |
| `renewAccessTokenOnExpiry` | Boolean | `true` | Whether to auto-refresh access token on expiry |
| `accessTokenExpiresLeeway` | Integer | `0` | Access token expiry leeway (seconds) |

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
| `jwksCacheTtl` | Integer | `300` | JWKS cache TTL (seconds) |
| `jwksMinRefreshInterval` | Integer | `10` | Minimum JWKS refresh interval (seconds) |
| `introspectionEndpoint` | String | none | Custom introspection endpoint URL |
| `introspectionEndpointAuthMethod` | Enum | `client_secret_basic` | Introspection endpoint auth method. Values: `client_secret_basic` | `client_secret_post` |
| `introspectionCacheTtl` | Integer | `0` | Introspection result cache TTL (seconds), `0` disables |
| `tokenEndpointAuthMethod` | Enum | `client_secret_basic` | Token endpoint auth method. Values: `client_secret_basic` | `client_secret_post` |

A failed discovery fetch is remembered for 5 seconds so concurrent or immediately following requests reuse that failure instead of contacting the identity provider again. The next call after that window retries; a successful document replaces the failure and is cached for the life of the plugin generation. This interval is not configurable.

### Cache Selection and Limits

Only positive introspection results use TinyUFO/S3-FIFO. Access-token
indirection retains the LRU cache because logout and refresh require explicit
invalidation:

```yaml
introspection:
  tinyUfoTtlCache:
    maxEntries: 2048
    maxLogicalBytes: 8MiB
accessToken:
  lruCache:
    capacity: 2048
    shards: 8
```

The two sections inherit `tiny_ufo_ttl_cache` and `lru_cache` Gateway defaults,
respectively. Set both TinyUFO override fields together, or omit both to inherit
the process defaults. For an explicit LRU capacity, `shards: 0` selects one
shard; a shards-only override is invalid. The TinyUFO byte field is the cache
instance's total logical record-weight budget, not an exact RSS limit. Discovery
and JWKS retain their dedicated snapshot providers; the refresh singleflight
registry is also separate from both caches.

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
dependency failures and are not governed by this credential-401 contract. Their
wire response follows the endpoint-specific error mapping; inspect the access-log
failure category rather than relying on one uniform status or body. Declared TLS
CA and mTLS material is consumed by the generation's canonical outbound client;
unresolved or malformed declared material follows the construction-time
fail-closed behavior described above, never these authentication-credential
fallback rules.

## Outbound egress

Every OpenID Connect outbound call — configured discovery and discovered JWKS, token, introspection, userinfo, and revocation endpoints — connects only to addresses allowed by the gateway outbound egress policy. Hostname destinations are classified by `EgressResolver` at dial time. IP-literal destinations are classified before reqwest connects, because reqwest skips custom DNS for literals. Both the default shared client and the SAN-pinning client ignore `HTTP_PROXY` / `HTTPS_PROXY` and do not follow redirects. Loopback, link-local, and unspecified destinations are always rejected. RFC1918 private destinations follow `outbound_egress_block_private` / `EDGION_OUTBOUND_BLOCK_PRIVATE` (default `false`), so a typical in-cluster identity provider remains reachable unless that switch is enabled. Denied destinations fail closed without including endpoints or resolved addresses in public bodies or PluginLog tags. JWKS may still serve a previously cached set when a later refresh fails; that stale-serve path is unchanged. TLS SNI, certificate verification, custom CA, and SAN pinning still use the original hostname. See [Egress policy (SSRF protection)](../../ops-guide/infrastructure/connectors.md#egress-policy-ssrf-protection).

## Security Defaults

- Tokens are not passed to upstream by default
- Header injection protection: rejects `\r`, `\n`, `\0`
- Header size limits:
  - `maxHeaderValueBytes` (default `4096`)
  - `maxTotalHeaderBytes` (default `16384`)
- Session cookie size limit: `maxSessionCookieBytes` (default `3800`)

## Common Troubleshooting

- `401 Unauthorized - Missing bearer token`
  - `bearerOnly=true` and the request does not carry `Authorization: Bearer ...`
- IdP dependency failure
  - Transport, response-decoding, and outbound TLS failures use endpoint-specific status/body mappings; inspect the access-log failure category
- `Session cookie exceeds configured size limit`
  - Reduce claims/header pass-through, or reduce the data stored in the session

## Related Documentation

- [JwtAuth Plugin](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT token authentication
- [ForwardAuth Plugin](forward-auth.md) - External authentication plugin
- [JWE Decrypt Plugin](jwe-decrypt.md) - JWE decryption authentication
