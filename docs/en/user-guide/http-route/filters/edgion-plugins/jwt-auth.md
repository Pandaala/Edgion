# JWT Auth Plugin

> **🔌 Edgion Extension**
> 
> JwtAuth is an authentication plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is JWT Auth?

JWT Auth (JSON Web Token Authentication) is a token-based authentication mechanism where clients carry a JWT in requests, and the gateway validates the signature and claims before allowing access.

**How it works**:
1. The client sends a request with a JWT (via Header, Query, or Cookie)
2. The plugin verifies the JWT signature, expiration time, and other claims
3. Verification succeeds: access is allowed, and the `X-Consumer-Username` header is set and passed to the upstream
4. Verification fails: a 401 status code is returned

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
  namespace: default
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRef:
          name: jwt-secret
        algorithm: HS256
```

### Create a Kubernetes Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-secret
  namespace: default
type: Opaque
stringData:
  secret: "my-jwt-secret-key-32-chars-long!!"
```

---

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `secretRef` | Object | None | Single-key mode: points to a Secret containing `secret` (HS*) or `publicKey` (RS*/ES*) |
| `secretRefs` | Array | None | Multi-key mode: each Secret must contain `key` (identifier) + `secret` or `publicKey` |
| `algorithm` | String | `HS256` | Signing algorithm, see supported algorithms below |
| `header` | String | `authorization` | Which Header to read the Token from (supports `Bearer <token>` or bare token) |
| `query` | String | `jwt` | Query parameter name |
| `cookie` | String | `jwt` | Cookie name |
| `hideCredentials` | Boolean | `false` | Whether to hide the Token from the upstream service |
| `anonymous` | String | None | Anonymous username. When set, unauthenticated requests are also allowed with this username |
| `keyClaimName` | String | `key` | Claim name in JWT payload used to select the key (multi-key mode) |
| `lifetimeGracePeriod` | Integer | `0` | Clock skew tolerance for exp/nbf (seconds) |

**Constraint**: Either `secretRef` or `secretRefs` must be configured.

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

```bash
curl "https://api.example.com/resource?jwt=eyJhbGciOiJIUzI1..."
```

### 3. Cookie

```bash
curl --cookie "jwt=eyJhbGciOiJIUzI1..." https://api.example.com/resource
```

---

## Common Configuration Scenarios

### 1. Single-Key Mode (HS256)

**Create Secret**:
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-secret
type: Opaque
stringData:
  secret: "my-super-secret-key-at-least-32-chars!"
```

**Configure plugin**:
```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRef:
          name: jwt-secret
        algorithm: HS256
        hideCredentials: true
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
        secretRef:
          name: jwt-rsa-public
        algorithm: RS256
```

### 3. Multi-Key Mode

Suitable for scenarios where multiple services/issuers use different keys to sign JWTs.

**Create multiple Secrets**:
```yaml
---
apiVersion: v1
kind: Secret
metadata:
  name: issuer-a-secret
type: Opaque
stringData:
  key: "issuer-a"           # Used to match the key claim in JWT payload
  secret: "issuer-a-secret-key-32-chars-long!!"
---
apiVersion: v1
kind: Secret
metadata:
  name: issuer-b-secret
type: Opaque
stringData:
  key: "issuer-b"
  secret: "issuer-b-secret-key-32-chars-long!!"
```

**Configure plugin**:
```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRefs:
          - name: issuer-a-secret
          - name: issuer-b-secret
        algorithm: HS256
        keyClaimName: key    # JWT payload must contain {"key": "issuer-a"} or {"key": "issuer-b"}
```

### 4. Anonymous Access Mode

Allow unauthenticated requests to pass through, but mark them as anonymous users:

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRef:
          name: jwt-secret
        anonymous: "guest"
```

**Behavior**:
- Valid Token provided: sets `X-Consumer-Username: <key_claim_value>`
- No Token or invalid Token: sets `X-Consumer-Username: guest` and `X-Anonymous-Consumer: true`

---

## Secret Data Format

### Single Key (secretRef)

| Algorithm Type | Required Field | Description |
|---------------|----------------|-------------|
| HS* | `secret` | HMAC shared secret (recommended 32+ bytes) |
| RS*/ES* | `publicKey` | PEM format public key |

### Multi-Key (secretRefs)

| Field | Description |
|-------|-------------|
| `key` | Identifier, matches the field corresponding to `keyClaimName` in the JWT payload |
| `secret` | Shared secret for HS* algorithms |
| `publicKey` | PEM format public key for RS*/ES* algorithms |

---

## Response Header Details

### Authentication Success

| Header Name | Description | Example |
|-------------|-------------|---------|
| `X-Consumer-Username` | Value of the `keyClaimName` field in the JWT payload | `user-123` |

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
  { key: 'my-user', exp: Math.floor(Date.now() / 1000) + 3600 },
  'my-super-secret-key-at-least-32-chars!',
  { algorithm: 'HS256' }
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
    {'key': 'my-user', 'exp': int(time.time()) + 3600},
    'my-super-secret-key-at-least-32-chars!',
    algorithm='HS256'
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

2. **Set reasonable expiration times**:
   ```json
   {"key": "user", "exp": 1735689600}  // exp is a UNIX timestamp
   ```

3. **Hide credentials**:
   ```yaml
   hideCredentials: true  # Don't pass the Token to upstream
   ```

4. **Use asymmetric algorithms**: For production, RS256 or ES256 is recommended — only the issuer holds the private key

5. **Rotate keys regularly**: Use multi-key mode for smooth key rotation

### Avoid

1. **Do not pass Tokens in URLs** (unless Headers/Cookies cannot be used)
2. **Do not use short keys** (HS256 recommends 32+ bytes)
3. **Do not ignore the exp claim**

---

## Troubleshooting

### Issue 1: Always Returns 401

**Possible causes**:
- Secret does not exist or has an incorrect name
- Secret is missing required fields (`secret` or `publicKey`)
- Algorithm does not match the key type

**Solution**:
```bash
kubectl get secret jwt-secret -o yaml
# Ensure it contains a secret or publicKey field
```

### Issue 2: Token Verification Fails

**Possible causes**:
- Key mismatch
- Token has expired (check the `exp` claim)
- Algorithm mismatch (the alg declared in the Token does not match the configuration)

**Solution**:
```bash
# Decode the Token to check its contents (without verifying the signature)
echo "eyJhbG..." | cut -d. -f2 | base64 -d
```

### Issue 3: Multi-Key Mode Cannot Find Key

**Possible causes**:
- JWT payload is missing the field corresponding to `keyClaimName`
- The value of `keyClaimName` does not match the `key` in the Secret

**Solution**:
Ensure the JWT payload contains:
```json
{"key": "issuer-a", "exp": 1735689600}
```
And the Secret's `key` field value is `issuer-a`.

---

## Complete Example

```yaml
# 1. Create Secret
apiVersion: v1
kind: Secret
metadata:
  name: api-jwt-secret
  namespace: default
type: Opaque
stringData:
  secret: "my-super-secret-key-for-jwt-auth-32!"
---
# 2. Create EdgionPlugins
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwt-auth-plugin
  namespace: default
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRef:
          name: api-jwt-secret
        algorithm: HS256
        header: authorization
        hideCredentials: true
---
# 3. Create HTTPRoute
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected-api
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
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
# Generate Token (example using Node.js)
TOKEN=$(node -e "console.log(require('jsonwebtoken').sign({key:'user1',exp:Math.floor(Date.now()/1000)+3600},'my-super-secret-key-for-jwt-auth-32!'))")

# Access with Token
curl -H "Authorization: Bearer $TOKEN" https://api.example.com/api/data

# No Token - returns 401
curl https://api.example.com/api/data
```
