# ResponseRewrite Plugin

> **🔌 Edgion Extension**
> 
> ResponseRewrite is a response rewriting plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is ResponseRewrite?

ResponseRewrite rewrites responses before returning them to the client, including:

- **Status code modification**: Modify the HTTP response status code
- **Response header operations**:
  - **set**: Set response headers (overwrite existing ones)
  - **add**: Add response headers (append to existing ones)
  - **remove**: Remove response headers
  - **rename**: Rename response headers (a rare feature among gateways)

## Quick Start

### Simplest Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-response-rewrite
spec:
  upstreamResponseFilterPlugins:
    - type: ResponseRewrite
      config:
        statusCode: 200
```

---

## Configuration Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `statusCode` | Integer | No | HTTP status code (100-599) |
| `headers` | Object | No | Response header modification operations |
| `headers.set` | Array | No | Set response headers (overwrite existing values) |
| `headers.add` | Array | No | Add response headers (append to existing values) |
| `headers.remove` | Array | No | Remove response headers |
| `headers.rename` | Array | No | Rename response headers |

### Headers Configuration Format

**set/add format**:
```yaml
headers:
  set:
    - name: "Header-Name"
      value: "Header-Value"
  add:
    - name: "Header-Name"
      value: "Header-Value"
```

**remove format**:
```yaml
headers:
  remove:
    - "Header-Name-1"
    - "Header-Name-2"
```

**rename format**:
```yaml
headers:
  rename:
    - from: "Old-Header-Name"
      to: "New-Header-Name"
```

---

## Configuration Scenarios

### 1. Modify Status Code

Uniformly change the upstream-returned status code:

```yaml
config:
  statusCode: 200
```

**Effect**: Regardless of what the upstream returns, the client receives `200 OK`

### 2. Set Response Headers

Set or overwrite response headers:

```yaml
config:
  headers:
    set:
      - name: Cache-Control
        value: "no-cache, no-store"
      - name: X-Content-Type-Options
        value: "nosniff"
```

**Effect**: Sets `Cache-Control` and `X-Content-Type-Options` response headers

### 3. Add Response Headers

Append new response headers (without overwriting existing values):

```yaml
config:
  headers:
    add:
      - name: X-Powered-By
        value: "Edgion"
      - name: X-Response-Time
        value: "50ms"
```

**Effect**: Adds `X-Powered-By` and `X-Response-Time` response headers

### 4. Remove Response Headers

Remove sensitive or unnecessary response headers:

```yaml
config:
  headers:
    remove:
      - Server
      - X-Powered-By
      - X-AspNet-Version
```

**Effect**: Removes `Server`, `X-Powered-By`, and `X-AspNet-Version` response headers

### 5. Rename Response Headers

Rename internal response headers to externally exposed names:

```yaml
config:
  headers:
    rename:
      - from: X-Internal-Request-Id
        to: X-Request-Id
      - from: X-Backend-Server
        to: X-Upstream-Server
```

**Effect**: `X-Internal-Request-Id` is renamed to `X-Request-Id`

### 6. Combined Configuration

Combining multiple rewriting features:

```yaml
config:
  statusCode: 200
  headers:
    rename:
      - from: X-Internal-Id
        to: X-Request-Id
    set:
      - name: Cache-Control
        value: "no-cache"
      - name: X-API-Version
        value: "v2"
    add:
      - name: X-Powered-By
        value: "Edgion"
    remove:
      - Server
      - X-Debug
```

---

## Execution Order

When multiple operations are configured simultaneously, the execution order is:

1. **Status code modification** (statusCode)
2. **Response header rename** (rename) — rename first to ensure subsequent operations use the new names
3. **Response header add** (add)
4. **Response header set** (set)
5. **Response header remove** (remove) — remove last to avoid deleting just-added headers

---

## Important Notes

### 1. Status Code Range

The status code must be in the 100-599 range:

```yaml
# ✅ Correct
config:
  statusCode: 201

# ❌ Wrong - out of range
config:
  statusCode: 600
```

### 2. How rename Works

`rename` copies the original header's value to the new name, then deletes the original header:

```yaml
config:
  headers:
    rename:
      - from: X-Old
        to: X-New
```

**Effect**: If the response contains `X-Old: value123`, it becomes `X-New: value123`

**Note**: If the original header does not exist, the `rename` operation is skipped without producing an error.

### 3. Response Header Name Case Sensitivity

HTTP response header names are case-insensitive, but using standard capitalization format is recommended (e.g., `Content-Type` rather than `content-type`).

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
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: api-response-rewrite
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-response-rewrite
  namespace: default
spec:
  upstreamResponseFilterPlugins:
    - type: ResponseRewrite
      config:
        # Uniformly return 200
        statusCode: 200
        headers:
          # Rename internal headers
          rename:
            - from: X-Internal-Request-Id
              to: X-Request-Id
          # Set cache and security headers
          set:
            - name: Cache-Control
              value: "no-cache, no-store"
            - name: X-Content-Type-Options
              value: "nosniff"
            - name: X-Frame-Options
              value: "DENY"
          # Add identifier
          add:
            - name: X-Powered-By
              value: "Edgion"
          # Remove sensitive headers
          remove:
            - Server
            - X-AspNet-Version
            - X-Debug
```

### Test

```bash
# Request
curl -i "https://api.example.com/api/users"

# Response (after rewriting):
# HTTP/1.1 200 OK
# X-Request-Id: abc123          (renamed from X-Internal-Request-Id)
# Cache-Control: no-cache, no-store
# X-Content-Type-Options: nosniff
# X-Frame-Options: DENY
# X-Powered-By: Edgion
# (Server header removed)
# (X-Debug header removed)
```

---

## Combining with Other Plugins

### With ProxyRewrite

Use ProxyRewrite to rewrite requests in the request phase, and ResponseRewrite to rewrite responses in the response phase:

```yaml
spec:
  requestPlugins:
    - type: ProxyRewrite
      config:
        uri: "/internal$uri"
        host: "backend.internal.svc"
  upstreamResponseFilterPlugins:
    - type: ResponseRewrite
      config:
        headers:
          remove:
            - Server
          add:
            - name: X-Gateway
              value: "Edgion"
```

### With Cors

ResponseRewrite can supplement CORS plugin response headers:

```yaml
spec:
  requestPlugins:
    - type: Cors
      config:
        allow_origins: "*"
        allow_methods: "GET,POST,PUT,DELETE"
  upstreamResponseFilterPlugins:
    - type: ResponseRewrite
      config:
        headers:
          set:
            - name: Access-Control-Max-Age
              value: "86400"
```

---

## Comparison with Other Gateways

| Feature | Edgion ResponseRewrite | APISIX response-rewrite | Kong response-transformer |
|---------|------------------------|-------------------------|---------------------------|
| Status code modification | ✅ | ✅ | ❌ |
| Response header set | ✅ | ✅ | ✅ (replace) |
| Response header add | ✅ | ✅ | ✅ |
| Response header remove | ✅ | ✅ | ✅ |
| Response header rename | ✅ | ❌ | ✅ |
| Body modification | ❌ | ✅ | ✅ (JSON) |
| Conditional matching | ❌ (phase 2) | ✅ | ❌ |

---

## Troubleshooting

### Issue 1: Response Headers Not Modified

**Check**:
1. Confirm EdgionPlugins is correctly associated with the HTTPRoute
2. Confirm the plugin type is `ResponseRewrite` not `ProxyRewrite`
3. Confirm response header names are spelled correctly

### Issue 2: rename Not Working

**Check**:
1. Confirm the original response header exists in the upstream response
2. Check the `from` field name is correct (case sensitivity)

### Issue 3: Configuration Validation Fails

**Common causes**:
1. `statusCode` is outside the 100-599 range
2. `name` or `from`/`to` in `headers` is empty

**Solution**: Check the configuration validation error messages in the gateway startup logs.

---

## Performance Notes

- **Synchronous execution**: ResponseRewrite executes synchronously in the `upstream_response_filter` phase, no asynchronous operations involved
- **Low latency**: Only modifies response header metadata, no response body processing
- **Memory efficient**: Does not buffer the response body, directly streaming transmission
