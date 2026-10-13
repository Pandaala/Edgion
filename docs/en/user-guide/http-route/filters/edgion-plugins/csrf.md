# CSRF Plugin

> **🔌 Edgion extension**
> 
> CSRF is a CSRF protection plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## What is CSRF?

CSRF (Cross-Site Request Forgery) is an attack method where an attacker tricks a user into performing unintended actions on a website where they are already authenticated.

**Attack example**:
1. The user logs into a bank website `bank.com`
2. The user visits a malicious website `evil.com`
3. `evil.com` sends a request to `bank.com/transfer?to=attacker&amount=1000`
4. Since the user is already logged in, the bank executes the transfer

**CSRF plugin protection mechanism**:
- Generates a unique random token for each user
- Safe methods (GET/HEAD/OPTIONS): automatically sets the token cookie
- Unsafe methods (POST/PUT/DELETE): verifies that the token in the request matches

## Quick Start

### Simplest Configuration

Create a Kubernetes Secret holding the signing key:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: csrf-signing-key
  namespace: default
type: Opaque
data:
  # base64-encoded signing key
  # Generate: openssl rand -base64 32 | tr -d '\n' | base64
  key: <base64-encoded-signing-key>
```

Reference it in the plugin config:

```yaml
filters:
  - type: Csrf
    config:
      secretRef:
        name: csrf-signing-key
        namespace: default
```

### Workflow

1. **Client's first visit (GET)**:
   ```bash
   curl https://api.example.com/api/data
   # Response includes: Set-Cookie: apisix-csrf-token=<token>
   ```

2. **Client submits a form (POST)**:
   ```bash
   curl -X POST https://api.example.com/api/submit \
     -H "apisix-csrf-token: <token>" \
     -H "Cookie: apisix-csrf-token=<token>" \
     -d "data=value"
   # Request succeeds
   ```

---

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `secretRef` | Object | none | **Required**. Reference to a Kubernetes Secret whose `key` data entry holds the CSRF signing key. |
| `expires` | Integer | `7200` (2 hours) | Token expiration time in seconds. Must be greater than zero; matching the session expiration time is recommended. |
| `name` | String | `"apisix-csrf-token"` | Token name used for both the cookie and request header. Must be a valid HTTP token of 1–256 bytes and must not be `cookie` in any letter case. |
| `cookieSecure` | Boolean | `true` | Whether to set the `Secure` attribute on the token cookie. Keep `true` when clients reach Edgion over HTTPS. Set `false` for HTTP-only deployment models (TLS terminated upstream at an LB/sidecar, mesh-internal plain HTTP, local development) — otherwise browsers drop the `Secure` cookie over HTTP and every state-mutating request fails with `no csrf cookie` |

---

## Common Configuration Scenarios

### 1. Basic Configuration (Recommended)

```yaml
filters:
  - type: Csrf
    config:
      secretRef:
        name: csrf-signing-key
        namespace: production
      expires: 7200  # 2 hours
```

### 2. Custom Token Name

```yaml
filters:
  - type: Csrf
    config:
      secretRef:
        name: csrf-signing-key
        namespace: production
      name: "x-csrf-token"  # Custom name
      expires: 3600  # 1 hour
```

### 3. Long-Lived Token

```yaml
filters:
  - type: Csrf
    config:
      secretRef:
        name: csrf-signing-key
        namespace: production
      expires: 86400  # 24 hours
```

⚠️ **Note**: The longer the expires time, the higher the risk of token theft.

---

## Client Integration

### HTML Form

```html
<!DOCTYPE html>
<html>
<head>
  <title>Submit Form</title>
</head>
<body>
  <form id="myForm" action="https://api.example.com/api/submit" method="POST">
    <input type="text" name="username" />
    <button type="submit">Submit</button>
  </form>

  <script>
    // Read token from cookie
    function getCookie(name) {
      const value = `; ${document.cookie}`;
      const parts = value.split(`; ${name}=`);
      if (parts.length === 2) return parts.pop().split(';').shift();
    }

    // Add CSRF token header when submitting the form
    document.getElementById('myForm').addEventListener('submit', function(e) {
      e.preventDefault();
      
      const token = getCookie('apisix-csrf-token');
      const formData = new FormData(this);
      
      fetch(this.action, {
        method: 'POST',
        headers: {
          'apisix-csrf-token': token  // Add token to header
        },
        body: formData,
        credentials: 'include'  // Send cookies
      }).then(response => {
        console.log('Success:', response);
      });
    });
  </script>
</body>
</html>
```

### JavaScript (Fetch API)

```javascript
// 1. First page load, get CSRF token
async function initCsrfToken() {
  await fetch('https://api.example.com/api/init', {
    credentials: 'include'  // Allow cookies
  });
  // Server will set the csrf token cookie in the response
}

// 2. Read token from cookie
function getCsrfToken() {
  const value = `; ${document.cookie}`;
  const parts = value.split('; apisix-csrf-token=');
  if (parts.length === 2) {
    return parts.pop().split(';').shift();
  }
  return null;
}

// 3. Send POST request
async function submitData(data) {
  const token = getCsrfToken();
  
  const response = await fetch('https://api.example.com/api/submit', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'apisix-csrf-token': token  // Add token
    },
    body: JSON.stringify(data),
    credentials: 'include'  // Send cookies
  });
  
  return response.json();
}

// Usage example
(async () => {
  await initCsrfToken();
  await submitData({ username: 'admin' });
})();
```

### React Example

```javascript
import React, { useState, useEffect } from 'react';
import axios from 'axios';

// Configure axios to automatically send cookies
axios.defaults.withCredentials = true;

function App() {
  const [csrfToken, setCsrfToken] = useState('');

  useEffect(() => {
    // Get CSRF token
    axios.get('https://api.example.com/api/init').then(() => {
      const token = getCsrfToken();
      setCsrfToken(token);
      
      // Set axios default header
      axios.defaults.headers.common['apisix-csrf-token'] = token;
    });
  }, []);

  const handleSubmit = async (e) => {
    e.preventDefault();
    
    // axios will automatically include cookies and headers
    const response = await axios.post('/api/submit', {
      username: 'admin'
    });
    
    console.log('Success:', response.data);
  };

  return (
    <form onSubmit={handleSubmit}>
      <input type="text" name="username" />
      <button type="submit">Submit</button>
    </form>
  );
}

function getCsrfToken() {
  const value = `; ${document.cookie}`;
  const parts = value.split('; apisix-csrf-token=');
  return parts.length === 2 ? parts.pop().split(';').shift() : null;
}

export default App;
```

### Vue.js Example

```javascript
import axios from 'axios';

// Global configuration
axios.defaults.withCredentials = true;

// Request interceptor: automatically add CSRF token
axios.interceptors.request.use(config => {
  const token = getCsrfToken();
  if (token) {
    config.headers['apisix-csrf-token'] = token;
  }
  return config;
});

function getCsrfToken() {
  const value = `; ${document.cookie}`;
  const parts = value.split('; apisix-csrf-token=');
  return parts.length === 2 ? parts.pop().split(';').shift() : null;
}

// Usage in component
export default {
  methods: {
    async submitForm() {
      await axios.post('/api/submit', {
        username: this.username
      });
    }
  }
}
```

---

## Security Best Practices

### ✅ Recommended

1. **Store the signing key in a Kubernetes Secret**
   ```bash
   # Generate a strong random key and create the Secret
   kubectl create secret generic csrf-signing-key \
     --from-literal=key="$(openssl rand -base64 32)" \
     -n production
   ```

2. **Rotate the signing key via the Secret**
   - Update the Secret's `key` data entry every 30-90 days
   - After the updated configuration reaches the Gateway, old CSRF tokens become invalid; clients must fetch a new token
   - If the Secret is deleted or cannot be resolved, the plugin fails closed until the key is available again
   - Use `kubectl create secret generic ... --dry-run=client -o yaml | kubectl apply -f -` for in-place rotation

3. **Restrict Secret access with RBAC**
   - Grant read access to the signing-key Secret only to the service accounts that require it
   - Audit Secret access via Kubernetes audit logs

4. **Coordinate with session management**
   ```yaml
   expires: 7200  # Match the session expiration time
   ```

5. **Use HTTPS**
   - CSRF tokens are transmitted via cookies, HTTPS is required
   - Cookies are set with `SameSite=Lax` for additional protection

### ❌ Avoid

1. **Do not put weak content in the signing-key Secret**
   ```bash
   # ❌ Dangerous — key value is too short or predictable
   kubectl create secret generic csrf-signing-key --from-literal=key="123456"
   ```

2. **Do not expose the signing key**
   - The signing key is stored in a Kubernetes Secret and accessed only server-side
   - The client only needs to know the CSRF token value, not the signing key

3. **Do not disable CSRF protection**
   ```yaml
   # ❌ Do not remove the CSRF plugin for convenience
   ```

---

## Request Method Handling

| Method | CSRF Check | Behavior |
|--------|-----------|----------|
| `GET` | ❌ Skipped | Automatically sets token cookie |
| `HEAD` | ❌ Skipped | Automatically sets token cookie |
| `OPTIONS` | ❌ Skipped | Automatically sets token cookie |
| `POST` | ✅ Validated | Must provide a valid token |
| `PUT` | ✅ Validated | Must provide a valid token |
| `DELETE` | ✅ Validated | Must provide a valid token |
| `PATCH` | ✅ Validated | Must provide a valid token |

**CSRF checks the method and token sources when the plugin executes.**
GET/HEAD/OPTIONS must lead to genuinely read-only operations; idempotence alone
does not make an operation safe. POST transport can also carry a read-only query.

### Method rewrites and plugin ordering

CSRF does not automatically recheck a request after ProxyRewrite or DSL changes
its method. `GET -> Csrf -> rewrite POST -> authenticated write` is outside the
protection guarantee: CSRF skips GET, and the later POST can execute without
token verification if authentication cookies accompany the request.

For a write operation, perform the rewrite before CSRF, preserve the client's
real token header and cookie, and ensure later plugins do not introduce an
unprotected write. Create the signing-key Secret shown above and attach this
resource through the HTTPRoute's ExtensionRef:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: protected-write
  namespace: default
spec:
  requestPlugins:
    - type: ProxyRewrite
      config:
        method: POST
        uri: /internal/action
    - type: Csrf
      config:
        secretRef:
          name: csrf-signing-key
```

GET becomes POST before this check: missing tokens return 401; matching, signed,
unexpired header and cookie tokens permit continuation. Obtain tokens beforehand
from a separate read-only GET endpoint using the same signing key and token name.
Do not copy a cookie into the token header within the chain: that defeats the
double-submit check.

Ordinary request plugins execute in order: GatewayClass-global
(`globalPluginsRef`), Gateway annotation (`edgion.io/edgion-plugins`), Route
filters, then selected BackendRef filters. Each chain follows declaration order.
Moving entries within a Route cannot make an earlier Global CSRF cover a later
Route or BackendRef rewrite. Put a CSRF check after the relevant rewrite,
including within the BackendRef chain when needed. Audit conditional execution,
token-source changes and all selected backends. This is an operator contract;
unsafe chains are not automatically validated or rejected.

For `POST -> rewrite GET -> Csrf`, CSRF skips verification; this is within the
contract only if the resulting operation is genuinely read-only.
For `POST -> Csrf -> rewrite GET`, CSRF requires a token even if the eventual
operation is read-only. Neither ordering proves that a GET endpoint cannot write.

RestToGraphql separately rejects entry GET mutations, including after an earlier
GET-to-POST rewrite. Entry GET queries can legitimately become POST GraphQL
transport and remain read-only. This adapter admission rule is not a general
CSRF final-method check. Audit the actual operation and complete chain.

---

## Error Responses

### Error 1: No token in request headers

```json
HTTP/1.1 401 Unauthorized
Content-Type: application/json

{
  "error_msg": "no csrf token in headers"
}
```

**Cause**: POST request did not include the CSRF token header

**Solution**:
```javascript
fetch('/api/submit', {
  method: 'POST',
  headers: {
    'apisix-csrf-token': token  // Add this line
  }
});
```

### Error 2: No token in cookie

```json
HTTP/1.1 401 Unauthorized

{
  "error_msg": "no csrf cookie"
}
```

**Cause**:
- Token was not obtained via a GET request on first visit
- Cookie was cleared or expired

**Solution**: First visit a GET endpoint to obtain the token

### Error 3: Token mismatch

```json
HTTP/1.1 401 Unauthorized

{
  "error_msg": "csrf token mismatch"
}
```

**Cause**: Token in the header does not match the token in the cookie

**Solution**: Ensure the token value read from the cookie is used in the header

### Error 4: Token signature verification failed

```json
HTTP/1.1 401 Unauthorized

{
  "error_msg": "Failed to verify the csrf token signature"
}
```

**Cause**:
- Token was tampered with
- Token has expired
- Server key has been changed

**Solution**: Obtain a new token

---

## Troubleshooting

### Issue 1: Cross-Origin Issues in Local Development

**Symptom**: Cookie cannot be set, SameSite error prompt

**Cause**:
- Frontend `http://localhost:3000`
- Backend `http://localhost:8080`
- Different ports are treated as cross-origin

**Solution**:
```yaml
# Configure CORS to allow credentials
filters:
  - type: Cors
    config:
      allowOrigins: "http://localhost:3000"
      allowCredentials: true  # Allow cookies
  
  - type: Csrf
    config:
      secretRef:
        name: csrf-signing-key
        namespace: default
```

### Issue 2: Mobile App Integration

**Symptom**: Mobile apps cannot use cookies

**Solution**:
- Option 1: Use WebView (supports cookies)
- Option 2: Use JWT authentication instead (more suitable for mobile)

### Issue 3: Token Expires Frequently

**Cause**: expires is set too short

**Solution**:
```yaml
filters:
  - type: Csrf
    config:
      secretRef:
        name: csrf-signing-key
        namespace: default
      expires: 14400  # Extend to 4 hours
```

---

## Combining with Other Plugins

### 1. With CORS

```yaml
filters:
  # Handle CORS first
  - type: Cors
    config:
      allowOrigins: "https://app.example.com"
      allowCredentials: true  # Must be enabled
      allowHeaders: "Content-Type,apisix-csrf-token"  # Allow CSRF header
  
  # Then validate CSRF
  - type: Csrf
    config:
      secretRef:
        name: csrf-signing-key
```

**Important**:
- `allowCredentials: true` is required (for cookies)
- `allowHeaders` must include the CSRF token name

### 2. With Basic Auth

```yaml
filters:
  # Authenticate first
  - type: BasicAuth
    config:
      secretGroups:
        - name: default
          secretRefs:
            - name: api-user

  # Then validate CSRF
  - type: Csrf
    config:
      secretRef:
        name: csrf-signing-key
```

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: web-api
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
            name: security-edgion_plugins
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: security-edgion_plugins
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST,PUT,DELETE"
        allowHeaders: "Content-Type,Authorization,apisix-csrf-token"
        allowCredentials: true
        maxAge: 86400
    - type: Csrf
      config:
        secretRef:
          name: csrf-signing-key
          namespace: production
        expires: 7200
        name: "apisix-csrf-token"
```

**Test**:
```bash
# 1. Get CSRF token (GET request)
curl -c cookies.txt https://api.example.com/api/users

# 2. Use token to submit data (POST request)
TOKEN=$(grep apisix-csrf-token cookies.txt | awk '{print $7}')
curl -X POST https://api.example.com/api/submit \
  -b cookies.txt \
  -H "apisix-csrf-token: $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"data":"value"}'
```

---

## Related Links

- [Cors](./cors.md) — Cross-Origin Resource Sharing
- [BasicAuth](./basic-auth.md) — Basic authentication
