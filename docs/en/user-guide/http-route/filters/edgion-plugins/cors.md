# CORS Plugin

> **🔌 Edgion Extension**
> 
> CORS is a cross-origin configuration plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is CORS?

CORS (Cross-Origin Resource Sharing) is a browser security mechanism that controls which websites can access your API.

**Simple example**:
- Your API is at `https://api.example.com`
- Your frontend is at `https://app.example.com`
- Without CORS configuration, the browser blocks frontend access to the API
- After configuring CORS, the browser allows cross-origin access

## Quick Start

### Simplest Configuration (Development Environment)

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

### Default Values

For security, the CORS plugin adopts a **deny-all** default policy — you must explicitly configure the allowed origins.

If `allow_origins` is not configured, all cross-origin requests will be rejected.

---

## Configuration Parameters

### Basic Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allow_origins` | String | `""` (empty) | **Required**. Allowed origin domains, separated by commas. Example: `"https://app.com,https://admin.com"` |
| `allow_origins_by_regex` | Array | None | Match origins using regular expressions. Example: `["^https://.*\\.example\\.com$"]` |
| `allow_methods` | String | `"GET,HEAD,OPTIONS"` | Allowed HTTP methods, separated by commas. Common: `"GET,POST,PUT,DELETE,PATCH"` |
| `allow_headers` | String | `"Accept,Accept-Language,Content-Language,Content-Type,Range"` | Allowed request headers, separated by commas. Common: `"Content-Type,Authorization"` |
| `expose_headers` | String | `""` (empty) | Response headers accessible to the browser. Example: `"X-Request-ID,X-Total-Count"` |

### Security Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allow_credentials` | Boolean | `false` | Whether to allow sending cookies and authentication information. Cannot use wildcard `*` when set to `true` |
| `max_age` | Integer | None | Cache duration for preflight requests (seconds). Recommended: `86400` (24 hours) |

### Advanced Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `preflight_continue` | Boolean | `false` | Whether to forward preflight requests to the upstream service. Usually keep `false` |
| `allow_private_network` | Boolean | `false` | Enable Private Network Access (Chrome 94+). Used for accessing private network resources from the public network |
| `timing_allow_origins` | String | None | Origins allowed to access the Resource Timing API. Same format as `allow_origins` |
| `timing_allow_origins_by_regex` | Array | None | Match Timing API origins using regular expressions |

### Special Values

| Value | Meaning | Use Case |
|-------|---------|----------|
| `*` | Wildcard, allows all | **Not recommended for production**. Use only in development environments |
| `**` | Force wildcard, bypasses security checks | **Dangerous**. Use only when fully understanding the risks |
| `""` (empty string) | Deny all | Default value, secure |

---

## Common Configuration Scenarios

### 1. Development Environment: Allow All Origins

⚠️ **For development only! Do not use in production!**

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "*"
      allow_methods: "*"
      allow_headers: "*"
```

### 2. Production Environment: Single Frontend Domain

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
      expose_headers: "X-Request-ID"
      max_age: 86400
```

### 3. Multiple Frontend Domains

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com,https://admin.example.com,https://mobile.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
      max_age: 86400
```

### 4. Allow All Subdomains

Using wildcard matching:

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "*.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

Or using regular expressions (more flexible):

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://example.com"  # Main domain
      allow_origins_by_regex:
        - "^https://.*\\.example\\.com$"    # All subdomains
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

### 5. Allow Local Development (localhost)

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_origins_by_regex:
        - "^http://localhost:[0-9]+$"        # localhost:any port
        - "^http://127\\.0\\.0\\.1:[0-9]+$"  # 127.0.0.1:any port
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

### 6. With Authentication (Cookie/Token)

⚠️ **Important**: When using `allow_credentials: true`, you **cannot** use the wildcard `*`

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"  # Must be a specific domain
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization,X-Custom-Token"
      allow_credentials: true                    # Allow sending cookies
      max_age: 86400
```

### 7. Restrict to Read-Only Access

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://public.example.com"
      allow_methods: "GET,HEAD,OPTIONS"  # Only allow read operations
      allow_headers: "Accept,Content-Type"
```

### 8. Complete RESTful API Configuration

```yaml
filters:
  - type: Cors
    config:
      # Allowed origins
      allow_origins: "https://app.example.com,https://admin.example.com"
      
      # RESTful methods
      allow_methods: "GET,POST,PUT,DELETE,PATCH,OPTIONS"
      
      # Common request headers
      allow_headers: "Content-Type,Authorization,X-Request-ID,X-Api-Key"
      
      # Expose custom response headers
      expose_headers: "X-Request-ID,X-Total-Count,X-Page-Count"
      
      # Enable authentication
      allow_credentials: true
      
      # Cache preflight results for 24 hours
      max_age: 86400
```

### 9. Performance Monitoring (Timing API)

If you need to allow third-party monitoring services to access performance data:

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST"
      allow_headers: "Content-Type"
      
      # Allow monitoring services to access the Resource Timing API
      timing_allow_origins: "https://analytics.example.com"
```

### 10. Private Network Access (Accessing Internal Networks)

Chrome 94+ requires additional configuration for accessing internal network resources from the public network:

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST"
      allow_headers: "Content-Type"
      allow_private_network: true  # Enable Private Network Access
```

---

## FAQ

### Q1: Why am I still getting CORS errors after configuration?

**A**: Check the following:

1. **Origin spelling is correct**: including protocol (http/https), domain, and port
   ```yaml
   ✅ Correct: "https://app.example.com"
   ❌ Wrong: "app.example.com" (missing protocol)
   ❌ Wrong: "https://app.example.com/" (trailing slash)
   ```

2. **Port number matches**:
   ```yaml
   ✅ "http://localhost:3000"  # Specific port
   ✅ "^http://localhost:[0-9]+$"  # Regex matching any port
   ```

3. **Cannot use wildcard with credentials**:
   ```yaml
   ❌ Wrong configuration:
   allow_origins: "*"
   allow_credentials: true
   
   ✅ Correct configuration:
   allow_origins: "https://app.example.com"
   allow_credentials: true
   ```

### Q2: Can I use `*` for development and specific domains for production?

**A**: Use environment variables or different configuration files:

```yaml
# development.yaml
filters:
  - type: Cors
    config:
      allow_origins: "*"
      allow_methods: "*"
      allow_headers: "*"

# production.yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

### Q3: How do I debug CORS issues?

**A**: Check the browser developer tools — Console and Network panels:

1. **Console** displays specific CORS error messages
2. **Network** panel to inspect request and response headers:
   - Request headers: `Origin`, `Access-Control-Request-Method`, `Access-Control-Request-Headers`
   - Response headers: `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers`

### Q4: What is a preflight request?

**A**: For certain "complex" requests, the browser sends an OPTIONS request first to ask the server if it allows the request:

**Triggers a preflight**:
- Using methods like `PUT`, `DELETE`, `PATCH`
- Using custom request headers (e.g., `X-Custom-Header`)
- `Content-Type` is `application/json`

**Does not trigger a preflight** (simple request):
- Using `GET`, `HEAD`, `POST` methods
- Only using basic request headers
- `Content-Type` is `application/x-www-form-urlencoded`, `multipart/form-data`, or `text/plain`

### Q5: What value should I set for `max_age`?

**A**: Recommended values:

```yaml
Development: max_age: 600       # 10 minutes, convenient for testing
Staging:     max_age: 3600      # 1 hour
Production:  max_age: 86400     # 24 hours, reduces preflight requests
```

### Q6: How do I write regular expressions?

**A**: Common regex examples:

```yaml
# All subdomains
"^https://.*\\.example\\.com$"

# localhost any port
"^http://localhost:[0-9]+$"

# Multiple top-level domains
"^https://app\\.(com|net|org)$"

# Development environment domains
"^https://.*\\.dev\\.example\\.com$"

# IP address range
"^http://192\\.168\\.1\\.[0-9]{1,3}:[0-9]+$"
```

**Note**: The `.` in regular expressions needs to be escaped as `\\.`

---

## Security Best Practices

### ✅ Recommended

1. **Principle of least privilege**: only allow necessary origins, methods, and headers
   ```yaml
   allow_origins: "https://app.example.com"  # Specific domain
   allow_methods: "GET,POST"                 # Only allow needed methods
   allow_headers: "Content-Type"             # Only allow needed headers
   ```

2. **No wildcards in production**:
   ```yaml
   ❌ allow_origins: "*"
   ✅ allow_origins: "https://app.example.com"
   ```

3. **Specific domains required with credentials**:
   ```yaml
   ✅ Secure:
   allow_origins: "https://app.example.com"
   allow_credentials: true
   ```

4. **Set a reasonable max_age**:
   ```yaml
   max_age: 86400  # 24 hours, balances performance and flexibility
   ```

5. **Only expose necessary response headers**:
   ```yaml
   expose_headers: "X-Request-ID"  # Only expose what's needed
   ```

### ❌ Avoid

1. **Using `*` in production**:
   ```yaml
   ❌ Dangerous:
   allow_origins: "*"
   allow_methods: "*"
   allow_headers: "*"
   ```

2. **Credentials with wildcards**:
   ```yaml
   ❌ Invalid configuration (will error):
   allow_origins: "*"
   allow_credentials: true
   ```

3. **Unnecessary methods**:
   ```yaml
   ❌ Overly permissive:
   allow_methods: "GET,POST,PUT,DELETE,PATCH,OPTIONS,TRACE,CONNECT"
   
   ✅ As needed:
   allow_methods: "GET,POST,PUT,DELETE"
   ```

---

## Performance Optimization Tips

1. **Use max_age to cache preflight requests**:
   ```yaml
   max_age: 86400  # Browser caches preflight results for 24 hours
   ```

2. **Avoid too many regular expressions**:
   ```yaml
   ❌ Poor performance:
   allow_origins_by_regex:
     - "^https://app1\\.example\\.com$"
     - "^https://app2\\.example\\.com$"
     - "^https://app3\\.example\\.com$"
   
   ✅ Good performance:
   allow_origins: "https://app1.example.com,https://app2.example.com,https://app3.example.com"
   ```

3. **Prefer `*.example.com` for subdomain wildcards**:
   ```yaml
   ✅ Fast:
   allow_origins: "*.example.com"
   
   ⚠️ Slower:
   allow_origins_by_regex:
     - "^https://.*\\.example\\.com$"
   ```

---

## Complete Configuration Example

### Typical Production Environment Configuration

```yaml
apiVersion: gateway.edgion.io/v1
kind: HTTPRoute
metadata:
  name: api-route
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
            value: /api/
      filters:
        - type: Cors
          config:
            # Allowed frontend domains
            allow_origins: "https://app.example.com,https://admin.example.com"
            
            # RESTful API methods
            allow_methods: "GET,POST,PUT,DELETE,PATCH,OPTIONS"
            
            # Allowed request headers
            allow_headers: "Content-Type,Authorization,X-Request-ID,X-Api-Version"
            
            # Exposed response headers (for frontend to read)
            expose_headers: "X-Request-ID,X-Total-Count,X-Rate-Limit-Remaining"
            
            # Allow sending cookies and tokens
            allow_credentials: true
            
            # Cache preflight requests for 24 hours
            max_age: 86400
      backendRefs:
        - name: api-service
          port: 8080
```

---

## Related Links

- [MDN: CORS Details](https://developer.mozilla.org/en-US/docs/Web/HTTP/CORS)
- [WHATWG Fetch Standard](https://fetch.spec.whatwg.org/)
- [Filters Overview](../overview.md)

---

## Getting Help

If you encounter issues:

1. Check the browser console for error messages
2. Review gateway logs: `logs/edgion_access.log` and `logs/edgion-gateway.log`
3. Refer to the FAQ section in this document
4. Submit an Issue on GitHub

**Remember**: CORS is a browser security mechanism. Once the server-side is configured correctly, the browser is responsible for validation and enforcement.
