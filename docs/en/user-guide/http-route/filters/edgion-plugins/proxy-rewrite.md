# ProxyRewrite Plugin

> **🔌 Edgion Extension**
> 
> ProxyRewrite is a request rewriting plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is ProxyRewrite?

ProxyRewrite rewrites requests before forwarding them to the upstream service, including:

- **URI rewriting**: Modify the request path
- **Host rewriting**: Modify the Host request header
- **Method rewriting**: Modify the HTTP method
- **Headers modification**: Add, set, or remove request headers

## Quick Start

### Simplest Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-proxy-rewrite
spec:
  requestPlugins:
    - enable: true
      type: ProxyRewrite
      config:
        uri: "/internal/api/v2"
```

---

## Configuration Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `uri` | String | No | New request path, supports variable substitution. Mutually exclusive with `regexUri`; `uri` has higher priority |
| `regexUri` | Object | No | Regex-based URI rewriting |
| `regexUri.pattern` | String | Yes | Regex match pattern |
| `regexUri.replacement` | String | Yes | Replacement template, supports `$1-$9` capture groups |
| `host` | String | No | New Host request header value |
| `method` | String | No | New HTTP method: GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS, etc. |
| `headers` | Object | No | Request header modification operations |
| `headers.add` | Array | No | Add request headers (append to existing values) |
| `headers.set` | Array | No | Set request headers (overwrite existing values) |
| `headers.remove` | Array | No | Remove request headers |

### Variable Reference

The following variables can be used in templates:

| Variable | Description | Example |
|----------|-------------|---------|
| `$uri` | Original request path | `/api/users` |
| `$arg_xxx` | Query parameter value | `$arg_id` → gets `123` from `?id=123` |
| `$1-$9` | Regex capture groups (`regexUri` scenarios only) | `/users/$1` |
| `$xxx` | Path parameters (extracted from `/:xxx` defined in HTTPRoute) | `$uid` |

**Note**: Query strings are automatically preserved — no manual handling needed.

---

## Configuration Scenarios

### 1. Simple URI Rewrite

Redirect all requests to a fixed path:

```yaml
config:
  uri: "/internal/api/v2"
```

**Effect**: `/api/users` → `/internal/api/v2`

### 2. Using the $uri Variable

Preserve the original path and add a prefix/suffix:

```yaml
config:
  uri: "/prefix$uri/suffix"
```

**Effect**: `/api/users` → `/prefix/api/users/suffix`

### 3. Using Query Parameter Variables

Extract values from query parameters to build a new path:

```yaml
config:
  uri: "/search/$arg_keyword/$arg_lang"
```

**Effect**: `/search?keyword=hello&lang=en` → `/search/hello/en`

### 4. Regex Rewriting

Using regex match and capture groups:

```yaml
config:
  regexUri:
    pattern: "^/api/v1/users/(\\d+)/profile"
    replacement: "/user-service/$1"
```

**Effect**: `/api/v1/users/123/profile` → `/user-service/123`

### 5. Multiple Capture Groups

```yaml
config:
  regexUri:
    pattern: "^/api/(\\w+)/(\\d+)"
    replacement: "/internal/$1/id/$2"
```

**Effect**: `/api/users/456` → `/internal/users/id/456`

### 6. Host Rewrite

Modify the request's Host header:

```yaml
config:
  host: "backend.internal.svc"
```

### 7. Method Rewrite

Convert a GET request to POST:

```yaml
config:
  method: "POST"
```

### 8. Headers Add

Append new request headers (without overwriting existing values):

```yaml
config:
  headers:
    add:
      - name: X-Gateway
        value: "edgion"
      - name: X-Request-Source
        value: "external"
```

### 9. Headers Set

Set request headers (overwrite existing values):

```yaml
config:
  headers:
    set:
      - name: X-Api-Version
        value: "v2"
      - name: X-Original-Path
        value: "$uri"
```

### 10. Headers Remove

Remove specified request headers:

```yaml
config:
  headers:
    remove:
      - X-Debug
      - X-Internal-Token
```

### 11. Path Parameter Extraction

When HTTPRoute uses path parameter patterns:

**HTTPRoute configuration**:
```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api/:uid/profile
```

**ProxyRewrite configuration**:
```yaml
config:
  uri: "/user-service/$uid/data"
  headers:
    set:
      - name: X-User-Id
        value: "$uid"
```

**Effect**: `/api/123/profile` → `/user-service/123/data`, with `X-User-Id: 123` set

### 12. Combined Configuration

Combining multiple rewriting features:

```yaml
config:
  uri: "/internal$uri"
  host: "backend.internal.svc"
  method: "POST"
  headers:
    add:
      - name: X-Gateway
        value: "edgion"
    set:
      - name: X-Original-Path
        value: "$uri"
      - name: X-Request-Id
        value: "req-12345"
    remove:
      - X-Debug
```

---

## Execution Order

When multiple rewriting operations are configured simultaneously, the execution order is:

1. **URI rewrite** (`uri` or `regexUri`)
2. **Host rewrite**
3. **Method rewrite**
4. **Headers modification** (add → set → remove)

---

## Important Notes

### 1. URI vs regexUri Priority

When both `uri` and `regexUri` are configured, **`uri` has higher priority** and `regexUri` will be ignored.

```yaml
# uri takes effect, regexUri is ignored
config:
  uri: "/new/path"
  regexUri:
    pattern: "^/api/(.*)"
    replacement: "/internal/$1"
```

### 2. Host Field Conflict

Do not set the Host header in both the `host` field and `headers.set`, as this will cause configuration validation to fail:

```yaml
# ❌ Wrong configuration
config:
  host: "backend.svc"
  headers:
    set:
      - name: Host
        value: "other.svc"

# ✅ Correct configuration
config:
  host: "backend.svc"
```

### 3. Query Parameters Auto-Preserved

After URI rewriting, the original request's query parameters are automatically appended:

```yaml
config:
  uri: "/new/path"
```

**Effect**: `/old/path?foo=bar&baz=qux` → `/new/path?foo=bar&baz=qux`

### 4. Variable URL Encoding

When `$arg_xxx` variables are used in the URI path, special characters are automatically URL-encoded (RFC 3986):

```yaml
config:
  uri: "/search/$arg_keyword"
```

**Effect**: `/search?keyword=hello world` → `/search/hello%20world`

### 5. Unmatched Path Parameters

If a `$name` variable is not defined in the route, the variable remains as-is without replacement:

```yaml
config:
  uri: "/api/$unknown/data"
```

**Effect**: `/test` → `/api/$unknown/data` (variable not replaced)

---

## Complete Example

### HTTPRoute + EdgionPlugins Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
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
            value: /api/v1
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: api-rewrite
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-rewrite
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: ProxyRewrite
      config:
        # URI rewrite: v1 → v2
        regexUri:
          pattern: "^/api/v1/(.*)"
          replacement: "/api/v2/$1"
        
        # Set internal Host
        host: "internal-api.default.svc"
        
        # Add gateway identifier
        headers:
          add:
            - name: X-Gateway
              value: "edgion"
          set:
            - name: X-Api-Version
              value: "v2"
            - name: X-Original-Uri
              value: "$uri"
          remove:
            - X-Debug
```

### Test

```bash
# Original request
curl -H "X-Debug: true" "https://api.example.com/api/v1/users/123?detail=true"

# Actual request forwarded to upstream:
# - URI: /api/v2/users/123?detail=true
# - Host: internal-api.default.svc
# - Headers:
#   - X-Gateway: edgion (added)
#   - X-Api-Version: v2 (set)
#   - X-Original-Uri: /api/v1/users/123 (set)
#   - X-Debug: (removed)
```

---

## Combining with Other Plugins

### With BasicAuth

Authenticate first, then rewrite:

```yaml
spec:
  requestPlugins:
    - enable: true
      type: BasicAuth
      config:
        secretRefs:
          - name: api-users
    - enable: true
      type: ProxyRewrite
      config:
        uri: "/internal$uri"
        headers:
          set:
            - name: X-Authenticated
              value: "true"
```

### With CORS

Handle cross-origin first, then rewrite:

```yaml
spec:
  requestPlugins:
    - enable: true
      type: Cors
      config:
        allow_origins: "https://app.example.com"
        allow_methods: "GET,POST,PUT,DELETE"
    - enable: true
      type: ProxyRewrite
      config:
        host: "backend.internal.svc"
```

---

## Troubleshooting

### Issue 1: URI Rewrite Not Taking Effect

**Check**:
1. Confirm `uri` or `regexUri` is configured correctly
2. If using `regexUri`, confirm the regex can match the request path
3. Check the gateway logs for rewrite records

### Issue 2: Variables Not Replaced

**Check**:
1. `$arg_xxx`: confirm the query parameter exists (case-sensitive)
2. `$name`: confirm the corresponding path parameter `/:name` is defined in the HTTPRoute
3. `$1-$9`: confirm the regex contains the corresponding number of capture groups

### Issue 3: Configuration Validation Fails

**Common causes**:
1. `host` field conflicts with Host in `headers.set`
2. `regexUri.pattern` has regex syntax errors

**Solution**: Check the configuration validation error messages in the gateway startup logs.

---

## Performance Notes

- **Regex pre-compilation**: `regexUri.pattern` is pre-compiled at configuration load time, with no additional overhead at runtime
- **Variable resolution**: Resolved on demand; unused variables are not processed
- **Path parameter extraction**: Uses lazy loading — extracted from the route pattern only on first access
