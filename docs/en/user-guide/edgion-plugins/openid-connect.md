# OpenID Connect Plugin

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

## Minimal Configuration Example (Bearer Only)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: oidc-api
  namespace: production
spec:
  requestPlugins:
    - plugin:
        type: OpenidConnect
        config:
          discovery: "https://idp.example.com/.well-known/openid-configuration"
          clientId: "my-api"
          bearerOnly: true
          verificationMode: JwksOnly
          unauthAction: Deny
      enabled: true
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
    - plugin:
        type: OpenidConnect
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
      enabled: true
```

## Secret Constraints

- `clientSecretRef`: reads from `clientSecret` / `client_secret` / `secret`
- `sessionSecretRef`: reads from `sessionSecret` / `session_secret` / `secret`
- `sessionSecret` should be at least 32 bytes (used for AES-256-GCM)

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
- `502 Failed to fetch OIDC discovery document`
  - `discovery` address is unreachable or TLS verification failed
- `Session cookie exceeds configured size limit`
  - Reduce claims/header pass-through, or reduce the data stored in the session
