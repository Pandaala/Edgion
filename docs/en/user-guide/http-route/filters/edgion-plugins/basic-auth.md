# Basic Auth Plugin

> **🔌 Edgion Extension**
> 
> BasicAuth is an authentication plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is Basic Auth?

Basic Auth is a simple HTTP authentication mechanism that requires clients to provide a username and password in the request header.

**How it works**:
1. The client sends a request with the `Authorization: Basic base64(username:password)` header
2. The plugin verifies the username and password
3. Verification succeeds: access is allowed, and the `X-Consumer-Username` header is set and passed to the upstream
4. Verification fails: a 401 status code is returned, requiring authentication

## Quick Start

### Simplest Configuration

```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: my-users-secret
      realm: "API Gateway"
```

### Create a Kubernetes Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: my-users-secret
  namespace: default
type: kubernetes.io/basic-auth
stringData:
  username: "admin"
  password: "secret123"
```

**Multiple users**: Create multiple Secrets and reference them in `secretRefs`:

```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: admin-user
        - name: api-user
        - name: readonly-user
```

---

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `secretRefs` | Array | None | **Required**. List of Kubernetes Secret references. Each Secret must be of type `kubernetes.io/basic-auth` and contain `username` and `password` fields |
| `realm` | String | `"API Gateway"` | Authentication realm name, displayed in the browser login dialog |
| `hideCredentials` | Boolean | `false` | Whether to hide the Authorization header from the upstream service |
| `anonymous` | String | None | Anonymous username. When set, unauthenticated requests are also allowed with this username |

---

## Common Configuration Scenarios

### 1. Basic Configuration: Single User

**Create Secret**:
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: api-user
type: kubernetes.io/basic-auth
stringData:
  username: "apiuser"
  password: "mySecretPassword123"
```

**Configure plugin**:
```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
      hideCredentials: true  # Don't pass credentials to upstream
```

### 2. Multi-User Configuration

**Create multiple Secrets**:
```yaml
---
apiVersion: v1
kind: Secret
metadata:
  name: admin-user
type: kubernetes.io/basic-auth
stringData:
  username: "admin"
  password: "adminPass123"
---
apiVersion: v1
kind: Secret
metadata:
  name: developer-user
type: kubernetes.io/basic-auth
stringData:
  username: "developer"
  password: "devPass456"
---
apiVersion: v1
kind: Secret
metadata:
  name: readonly-user
type: kubernetes.io/basic-auth
stringData:
  username: "readonly"
  password: "readPass789"
```

**Configure plugin**:
```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: admin-user
        - name: developer-user
        - name: readonly-user
      realm: "My API"
      hideCredentials: true
```

### 3. Anonymous Access Mode

Allow unauthenticated requests to pass through, but mark them as anonymous users:

```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: premium-user
      anonymous: "guest"  # Mark unauthenticated users as "guest"
```

**Behavior**:
- Correct credentials provided: sets `X-Consumer-Username: premium-user`
- No credentials provided: sets `X-Consumer-Username: guest` and `X-Anonymous-Consumer: true`

### 4. Custom Authentication Realm

```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
      realm: "Protected API - Please Login"  # Prompt displayed in the browser
```

---

## Client Usage Examples

### cURL

```bash
# Method 1: Using the -u parameter
curl -u username:password https://api.example.com/resource

# Method 2: Manually constructing the Authorization header
curl -H "Authorization: Basic $(echo -n 'username:password' | base64)" \
  https://api.example.com/resource
```

### JavaScript (Fetch API)

```javascript
const username = 'myuser';
const password = 'mypass';
const credentials = btoa(`${username}:${password}`);

fetch('https://api.example.com/resource', {
  headers: {
    'Authorization': `Basic ${credentials}`
  }
});
```

### Python (requests)

```python
import requests

response = requests.get(
    'https://api.example.com/resource',
    auth=('username', 'password')
)
```

---

## Response Header Details

### Authentication Success

The plugin automatically sets the following request headers to pass to the upstream:

| Header Name | Description | Example |
|-------------|-------------|---------|
| `X-Consumer-Username` | Username of the authenticated user | `admin` |
| `X-Anonymous-Consumer` | Whether the user is anonymous | `true` (anonymous mode only) |

### Authentication Failure

Returns **401 Unauthorized** with the following response headers:

```
HTTP/1.1 401 Unauthorized
WWW-Authenticate: Basic realm="API Gateway"
Content-Type: text/plain

401 Unauthorized - Authentication required
```

---

## Security Best Practices

### ✅ Recommended

1. **Always use HTTPS**
   ```yaml
   # Basic Auth transmits plaintext credentials (base64 encoded), HTTPS is required
   ```

2. **Use strong passwords**
   - Minimum 12 characters
   - Include uppercase/lowercase letters, numbers, and special characters
   - Avoid common passwords

3. **Hide credentials**
   ```yaml
   hideCredentials: true  # Don't pass the Authorization header to upstream
   ```

4. **Rotate passwords regularly**
   ```bash
   # Update Secret
   kubectl create secret generic api-user \
     --from-literal=username=apiuser \
     --from-literal=password=newPassword123 \
     --dry-run=client -o yaml | kubectl apply -f -
   ```

5. **Principle of least privilege**
   - Create different users for different permissions
   - Read-only users should use read-only credentials

### ❌ Avoid

1. **Do not use Basic Auth over HTTP**
   ```yaml
   # ❌ Dangerous: credentials will be transmitted in plaintext
   # ✅ Must use HTTPS
   ```

2. **Do not hardcode passwords in code**
   ```yaml
   # ❌ Don't do this
   # password: "hardcoded_password"
   
   # ✅ Use Kubernetes Secrets
   secretRefs:
     - name: user-secret
   ```

3. **Do not use Basic Auth for public APIs**
   - Basic Auth is suitable for internal APIs or admin interfaces
   - Public APIs should use OAuth 2.0 or JWT

---

## Troubleshooting

### Issue 1: Always Returns 401

**Cause**:
- Secret does not exist or has an incorrect name
- Secret type is not `kubernetes.io/basic-auth`
- Secret does not contain `username` and `password` fields

**Solution**:
```bash
# Check the Secret
kubectl get secret api-user -o yaml

# Ensure the type is correct
type: kubernetes.io/basic-auth

# Ensure required fields are present
data:
  username: ...
  password: ...
```

### Issue 2: Password Verification Fails

**Cause**:
- Password mismatch
- Password was base64 encoded (should use the raw password)

**Solution**:
```yaml
# ✅ Correct: use stringData (auto-encodes)
stringData:
  username: "admin"
  password: "myPassword123"

# ❌ Wrong: manually base64 encoded
data:
  username: YWRtaW4=
  password: bXlQYXNzd29yZDEyMw==  # Will be encoded again
```

### Issue 3: Upstream Receives the Authorization Header

**Cause**: `hideCredentials: true` was not set

**Solution**:
```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
      hideCredentials: true  # Add this line
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
      allowCredentials: true  # Required for Basic Auth
      allowHeaders: "Content-Type,Authorization"  # Allow the Authorization header
  
  # Then authenticate
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
```

**Note**: The CORS plugin must allow the `Authorization` header.

### 2. With IP Restriction

```yaml
filters:
  # Restrict IP first
  - type: IpRestriction
    config:
      allow: ["10.0.0.0/8", "192.168.0.0/16"]
  
  # Then authenticate
  - type: BasicAuth
    config:
      secretRefs:
        - name: internal-api-user
```

**Advantage**: Dual protection — only requests from internal IPs with correct credentials can access the resource.

---

## Performance Considerations

- **Password hashing**: The plugin uses bcrypt to hash passwords (computed at startup), providing good runtime verification performance
- **Concurrent requests**: Each request undergoes password verification; for high-concurrency scenarios, consider using caching
- **Secret updates**: After modifying a Secret, the plugin automatically reloads (there may be a brief delay)

---

## Limitations

1. **Only supports Kubernetes Secrets**
   - Does not support file or environment variable configuration
   - Must use the `kubernetes.io/basic-auth` type

2. **No dynamic permissions**
   - All users have the same access permissions
   - For fine-grained access control, implement it in the upstream service

3. **No user management API**
   - Users must be managed through the Kubernetes API
   - Does not provide user registration, password reset, or similar features

---

## Complete Example

### Gateway API HTTPRoute Configuration

```yaml
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
            name: auth-edgion_plugins
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-edgion_plugins
  namespace: default
spec:
  plugins:
    - enable: true
      plugin:
        type: BasicAuth
        config:
          secretRefs:
            - name: api-admin
            - name: api-developer
          realm: "Protected API"
          hideCredentials: true
---
apiVersion: v1
kind: Secret
metadata:
  name: api-admin
type: kubernetes.io/basic-auth
stringData:
  username: "admin"
  password: "AdminSecret123!"
---
apiVersion: v1
kind: Secret
metadata:
  name: api-developer
type: kubernetes.io/basic-auth
stringData:
  username: "developer"
  password: "DevSecret456!"
```

**Test**:
```bash
# Using admin user
curl -u admin:AdminSecret123! https://api.example.com/api/users

# Using developer user
curl -u developer:DevSecret456! https://api.example.com/api/data

# No credentials - returns 401
curl https://api.example.com/api/users
```
