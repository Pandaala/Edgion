# JWT Auth Plugin

> **🔌 Edgion extension**
> 
> JwtAuth is an authentication plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is JWT Auth?

JWT Auth (JSON Web Token Authentication) is a token-based authentication mechanism where clients carry a JWT in requests, and the gateway validates the signature and claims before allowing access.

**How it works**:
1. The client sends a request with a JWT (via Header, Query, or Cookie)
2. The plugin decodes the JWT `iss` (issuer) claim and routes to the matching issuer group
3. The JWT signature, expiration, and other claims are verified using that group's algorithm and keys
4. Verification succeeds: access is allowed, the `X-Consumer-Username` header is set to the `sub` claim, and the access log records `OK group=<name> iss=<iss> u=<sub>`
5. Verification fails: a 401 status code is returned

**Differences from BasicAuth**:
- BasicAuth: transmits username and password with every request
- JwtAuth: only verifies the token, no password transmission; supports stateless authentication; tokens have expiration times

## Quick Start

### Simplest Configuration

```yaml
# EdgionPlugins configuration
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwt-auth-plugin
  namespace: production
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        issuerGroups:
          - name: default
            issuers:
              - "https://my-idp.example.com"
            secretRefs:
              - name: jwt-secret
```

### Create a Kubernetes Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-secret
  namespace: production
type: Opaque
stringData:
  key: "my-key-id"
  secret: "my-jwt-secret-key-32-chars-long!!"
```

---

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `issuerGroups` | Array | none | Named issuer groups (see sub-table below). Required when `anonymous` is not set |
| `algorithm` | String | `HS256` | Plugin-level default JWT signing algorithm; overridable per issuerGroup |
| `audiences` | Array | none | Plugin-level default valid audiences; overridable per issuerGroup |
| `header` | String | `authorization` | Header name to read the token from (supports `Bearer <token>` or bare token) |
| `query` | String | `jwt` | Query parameter name for the token. Only consulted when `allowTokenInQuery` is `true` |
| `allowTokenInQuery` | Boolean | `false` | Allow reading the token from the URL query parameter. Disabled by default because bearer tokens in URLs leak into access logs, browser history, and `Referer` headers (RFC 6750 §2.3/§5.3) |
| `cookie` | String | `jwt` | Cookie name for the token |
| `hideCredentials` | Boolean | `false` | Whether to remove the token from the request before forwarding to upstream |
| `anonymous` | String | none | Anonymous username. When set, unauthenticated requests are allowed and assigned this username |
| `keyClaimName` | String | `key` | Claim name in the JWT payload used to select the credential key |
| `lifetimeGracePeriod` | Integer | `0` | Clock skew tolerance for exp/nbf validation (seconds) |
| `base64Secret` | Boolean | `false` | Whether the HS* secret value is base64-encoded |
| `maximumExpiration` | Integer | `0` | Maximum allowed exp duration from now (seconds). `0` disables the check |
| `storeClaimsInCtx` | Boolean | `false` | Store decoded claims in the ctx variable `jwt_claims` (read by downstream plugins via `get_ctx_var`; excluded from the access log so claims are not persisted) |
| `claimsToHeaders` | Object | none | Map JWT claim names to upstream request header names |

**Constraint**: `issuerGroups` is required when `anonymous` is not set.

### JwtIssuerGroup Sub-table

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Group name; charset follows K8s label-value (`^[a-zA-Z0-9]([-_.a-zA-Z0-9]*[a-zA-Z0-9])?$`, max 63 chars). Surfaces in the access log as `group=<name>` |
| `description` | String | No | Human-readable description; not used at runtime |
| `issuers` | Array | Yes | Issuer values this group covers (the JWT `iss` claim values). Must be unique across all groups |
| `secretRefs` | Array | Yes | Non-empty list of Secret references for this group. Each Secret must contain `key` + `secret` (HS*) or `publicKey` (RS*/ES*) |
| `algorithm` | String | No | Override the plugin-level `algorithm`. Falls back to the plugin-level default when not set |
| `audiences` | Array | No | Override the plugin-level `audiences`. Set to `[]` to explicitly disable the audience check for this group |

---

## Supported Algorithms

| Type | Algorithms | Description |
|------|-----------|-------------|
| Symmetric (HMAC) | `HS256`, `HS384`, `HS512` | Signed with a shared secret. Secret must contain a `secret` field |
| Asymmetric (RSA) | `RS256`, `RS384`, `RS512` | Verified with an RSA public key. Secret must contain a `publicKey` field (PEM format) |
| Asymmetric (ECDSA) | `ES256`, `ES384` | Verified with an ECDSA public key. Secret must contain a `publicKey` field (PEM format) |

> **Note**: ES512 (P-521) is not currently supported due to underlying library limitations.

---

## Token Delivery Methods

The plugin extracts tokens with the following priority: **Header > Query > Cookie**

### 1. Authorization Header (Recommended)

```bash
curl -H "Authorization: Bearer eyJhbGciOiJIUzI1..." https://api.example.com/resource
```

### 2. Query Parameter

> **Disabled by default.** Bearer tokens in URLs leak into access logs, browser history, and `Referer` headers (RFC 6750 §2.3/§5.3). To use this method you must explicitly opt in with `allowTokenInQuery: true`; otherwise a token carried only in the query is ignored and the request is treated as unauthenticated.

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        allowTokenInQuery: true   # required to read the token from the query
        # ...
```

```bash
curl "https://api.example.com/resource?jwt=eyJhbGciOiJIUzI1..."
```

### 3. Cookie

```bash
curl --cookie "jwt=eyJhbGciOiJIUzI1..." https://api.example.com/resource
```

---

## Organizing Issuers with issuerGroups

`issuerGroups` is the core of JwtAuth: each JWT's `iss` claim determines which group's keys and algorithm are used for verification, fully isolating credentials across different issuers.

### Single-issuer Configuration

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        issuerGroups:
          - name: my-idp
            issuers:
              - "https://my-idp.example.com"
            secretRefs:
              - name: my-idp-secret
```

### Multi-issuer Configuration (partner-a + partner-b)

Suitable when multiple partners sign JWTs with different keys:

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256   # plugin-level default, overridable per group
        issuerGroups:
          - name: partner-a
            issuers:
              - "https://partner-a.example.com"
            secretRefs:
              - name: partner-a-secret
            # inherits plugin-level algorithm: HS256
          - name: partner-b
            issuers:
              - "https://partner-b.example.com"
            secretRefs:
              - name: partner-b-secret
            algorithm: HS256   # explicit override of plugin-level default
            audiences:
              - "my-api.example.com"   # audience check scoped to this group
```

**Routing logic**:
1. The gateway decodes the JWT payload (without signature verification) to extract the `iss` claim
2. The matching group is located by `iss` in `issuerGroups`
3. `header.alg` must equal the group's `algorithm` (algorithm-confusion defense)
4. Full signature verification is performed using the group's keys and algorithm

**Group-level `algorithm` / `audiences` override semantics**:
- `algorithm`: when not set, inherits the plugin-level default; when set, overrides it
- `audiences`: when not set, inherits the plugin-level default; set to `[]` to explicitly disable audience validation for this group

### Access Log Format

On successful verification, the access log records:

```
OK group=partner-a iss=https://partner-a.example.com u=user-123
```

Where:
- `group=<name>`: the issuer group matched for this token
- `iss=<iss>`: the raw issuer claim value from the JWT
- `u=<sub>`: the `sub` (subject) claim value from the JWT

### Security Note: Failure Branches Omit group/iss

**On verification failure, the access log does not include `group=` or `iss=` values** (by design).

Rationale: an unauthenticated attacker must not be able to enumerate which issuers are configured, infer the algorithm used by a given issuer, or probe the kid distribution across groups by observing error responses — these pieces of information together can map the tenant topology.

Failure branch examples:

- `Deny malformed token` — token format is invalid
- `Deny missing iss` — the token has no `iss` claim
- `Deny unknown iss` — the `iss` does not match any configured group
- `Deny bad algorithm` — the token header `alg` does not match the group's algorithm
- `Deny missing key` — the kid is not in the routed group's credentials
- `Deny bad signature` — signature verification failed
- `Deny exp` / `Deny nbf` / `Deny bad aud` — claim validation failures

---

## Common Configuration Scenarios

### 1. HS256 Single Key

**Create Secret**:
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-secret
type: Opaque
stringData:
  key: "default-key"
  secret: "my-super-secret-key-at-least-32-chars!"
```

**Configure plugin**:
```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        hideCredentials: true
        issuerGroups:
          - name: default
            issuers:
              - "https://my-idp.example.com"
            secretRefs:
              - name: jwt-secret
```

### 2. RSA Public Key Mode (RS256)

**Create Secret (with PEM format public key)**:
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-rsa-public
type: Opaque
stringData:
  key: "rsa-key"
  publicKey: |
    -----BEGIN PUBLIC KEY-----
    MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA...
    -----END PUBLIC KEY-----
```

**Configure plugin**:
```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: RS256
        issuerGroups:
          - name: rsa-idp
            issuers:
              - "https://rsa-idp.example.com"
            secretRefs:
              - name: jwt-rsa-public
```

### 3. Anonymous Access Mode

Allow unauthenticated requests through, but mark them as anonymous users:

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        anonymous: "guest"
        issuerGroups:
          - name: default
            issuers:
              - "https://my-idp.example.com"
            secretRefs:
              - name: jwt-secret
```

**Behavior**:
- Valid Token provided: sets `X-Consumer-Username: <sub>`
- No Token or invalid Token: sets `X-Consumer-Username: guest` and `X-Anonymous-Consumer: true`

---

## Secret Data Format

Each Secret referenced by an `issuerGroup` must contain the following fields:

| Algorithm Type | Required Fields | Description |
|---------------|-----------------|-------------|
| HS* | `key` | Identifier matching the `keyClaimName` field in the JWT payload |
| HS* | `secret` | HMAC shared secret (recommended 32+ bytes) |
| RS*/ES* | `key` | Identifier matching the `keyClaimName` field in the JWT payload |
| RS*/ES* | `publicKey` | PEM format public key |

---

## Response Header Details

### Authentication Success

| Header Name | Description | Example |
|-------------|-------------|---------|
| `X-Consumer-Username` | Value of the `sub` claim in the JWT payload | `user-123` |

### Authentication Failure

Returns **401 Unauthorized**:

```
HTTP/1.1 401 Unauthorized
Content-Type: text/plain

401 Unauthorized - Invalid or missing JWT
```

---

## Client Usage Examples

### JavaScript (Generating JWT)

```javascript
// Using the jsonwebtoken library
const jwt = require('jsonwebtoken');

const token = jwt.sign(
  {
    sub: 'my-user',
    iss: 'https://my-idp.example.com',
    exp: Math.floor(Date.now() / 1000) + 3600
  },
  'my-super-secret-key-at-least-32-chars!',
  { algorithm: 'HS256', keyid: 'default-key' }
);

fetch('https://api.example.com/resource', {
  headers: { 'Authorization': `Bearer ${token}` }
});
```

### Python

```python
import jwt
import time

token = jwt.encode(
    {
        'sub': 'my-user',
        'iss': 'https://my-idp.example.com',
        'exp': int(time.time()) + 3600
    },
    'my-super-secret-key-at-least-32-chars!',
    algorithm='HS256',
    headers={'kid': 'default-key'}
)

import requests
response = requests.get(
    'https://api.example.com/resource',
    headers={'Authorization': f'Bearer {token}'}
)
```

### cURL

```bash
# Assuming you already have a Token
TOKEN="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."

# Header method
curl -H "Authorization: Bearer $TOKEN" https://api.example.com/resource

# Query method
curl "https://api.example.com/resource?jwt=$TOKEN"
```

---

## Security Best Practices

### Recommended

1. **Always use HTTPS**: JWTs should not be intercepted in transit

2. **Create a separate issuerGroup for each issuer**: isolates keys and prevents cross-issuer credential confusion

3. **Set reasonable expiration times**:
   ```json
   {"sub": "user", "iss": "https://my-idp.example.com", "exp": 1735689600}
   ```

4. **Hide credentials**:
   ```yaml
   hideCredentials: true  # Do not forward the token to upstream
   ```

5. **Use asymmetric algorithms**: For production, RS256 or ES256 is recommended — only the issuer holds the private key

6. **Configure audiences**: Specify the allowed audience list to prevent JWTs from being reused by other services

### Avoid

1. **Do not pass Tokens in URLs** — the query method is off by default (`allowTokenInQuery: false`); only enable it when Headers/Cookies genuinely cannot be used
2. **Do not use short keys** (HS256 recommends 32+ bytes)
3. **Do not ignore the exp claim**
4. **Do not share a single issuerGroup across multiple issuers** — each issuer should have its own group

---

## Troubleshooting

### Issue 1: Always Returns 401

**Possible causes**:
- Secret does not exist or has an incorrect name
- Secret is missing required fields (`secret` or `publicKey`)
- Algorithm does not match the key type
- JWT `iss` claim does not match any issuerGroup's `issuers` list

**Solution**:
```bash
kubectl get secret jwt-secret -o yaml
# Ensure it contains key + secret or key + publicKey fields
```

### Issue 2: Token Verification Fails

**Possible causes**:
- Key mismatch
- Token has expired (check the `exp` claim)
- Algorithm mismatch (the alg in the token header does not match the issuerGroup's algorithm)
- JWT is missing the `iss` claim

**Solution**:
```bash
# Decode the Token to check its contents (without verifying the signature)
echo "eyJhbG..." | cut -d. -f2 | base64 -d
```

### Issue 3: Multi-issuer Mode Cannot Find the Matching Group

**Possible causes**:
- The JWT `iss` value does not exactly match any `issuers` entry in any issuerGroup (including trailing slash differences)
- An issuer value is duplicated across groups (each `iss` must be unique across all groups)

**Solution**:
Ensure the `iss` claim in the JWT payload exactly matches the configured `issuers` value (exact string match, including protocol scheme and path).

---

## Complete Example

```yaml
# 1. Create Secret (partner-a)
apiVersion: v1
kind: Secret
metadata:
  name: partner-a-secret
  namespace: production
type: Opaque
stringData:
  key: "partner-a-key"
  secret: "partner-a-secret-key-32-chars-long!!"
---
# 2. Create Secret (partner-b)
apiVersion: v1
kind: Secret
metadata:
  name: partner-b-secret
  namespace: production
type: Opaque
stringData:
  key: "partner-b-key"
  secret: "partner-b-secret-key-32-chars-long!!"
---
# 3. Create EdgionPlugins
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwt-auth-plugin
  namespace: production
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        header: authorization
        hideCredentials: true
        issuerGroups:
          - name: partner-a
            issuers:
              - "https://partner-a.example.com"
            secretRefs:
              - name: partner-a-secret
          - name: partner-b
            issuers:
              - "https://partner-b.example.com"
            secretRefs:
              - name: partner-b-secret
            audiences:
              - "my-api.example.com"
---
# 4. Create HTTPRoute
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected-api
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: jwt-auth-plugin
      backendRefs:
        - name: backend-service
          port: 8080
```

**Test**:
```bash
# Generate a Token for partner-a (example using Node.js)
TOKEN=$(node -e "console.log(require('jsonwebtoken').sign({sub:'user1',iss:'https://partner-a.example.com',exp:Math.floor(Date.now()/1000)+3600},'partner-a-secret-key-32-chars-long!!',{keyid:'partner-a-key'}))")

# Access with Token (access log will record: OK group=partner-a iss=https://partner-a.example.com u=user1)
curl -H "Authorization: Bearer $TOKEN" https://api.example.com/api/data

# No Token - returns 401
curl https://api.example.com/api/data
```

---

## Related Links

- [BasicAuth](./basic-auth.md) — Basic authentication
- [KeyAuth](./key-auth.md) — API Key authentication
- [HmacAuth](./hmac-auth.md) — HMAC authentication
