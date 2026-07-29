# ProxyRewrite Plugin

> **🔌 Edgion extension**
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
    - type: ProxyRewrite
      config:
        uri: "/internal/api/v2"
```

---

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `uri` | String | none | New request path, supports variable substitution. Mutually exclusive with `regexUri`; `uri` has higher priority |
| `regexUri` | Object | none | Regex-based URI rewriting |
| `regexUri.pattern` | String | **Required** (within `regexUri`) | Regex match pattern |
| `regexUri.replacement` | String | **Required** (within `regexUri`) | Replacement template, supports `$1-$9` capture groups |
| `host` | String | none | New Host request header value |
| `method` | String | none | New HTTP method: GET / POST / PUT / DELETE / PATCH / HEAD / OPTIONS, etc. |
| `headers` | Object | none | Request header modification operations |
| `headers.add` | Array\<Object\> | none | Add request headers (append to existing values) |
| `headers.set` | Array\<Object\> | none | Set request headers (overwrite existing values) |
| `headers.remove` | Array\<String\> | none | Remove request headers |
| `jsonBody` | Object | none | Structurally rewrite a buffered JSON request body |
| `jsonBody.operations` | Array\<Object\> | **Required** | Ordered RFC 6902 operations: `add`, `remove`, `replace`, `move`, `copy`, or `test` |

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

### Regex rewrite (`ProxyRewriteRegex` mode)

> **Note on `ProxyRewriteRegex`**: There is **no separate `ProxyRewriteRegex`
> plugin type**. Regex-based URI rewriting is the `regexUri` mode of the single
> `ProxyRewrite` plugin (`type: ProxyRewrite`). The capture-group rewrite shown
> in scenarios 4 and 5 above *is* the "ProxyRewriteRegex" behavior. Use the
> `uri` field for a static path and the `regexUri` field (with `pattern` +
> `replacement`) when you need to capture parts of the incoming path.

The two URI-rewrite modes are mutually exclusive in effect — when both `uri` and
`regexUri` are set, `uri` wins and `regexUri` is ignored:

| Mode | Field | When to use |
|------|-------|-------------|
| Static | `uri` | The upstream path is a fixed string (optionally with `$uri` / `$arg_*` variables). |
| Regex | `regexUri.pattern` + `regexUri.replacement` | The upstream path must capture segments of the request path via `$1`, `$2`, … |

```yaml
# Capture and reshape the path, and inject a marker header.
config:
  regexUri:
    pattern: "^/api/v[0-9]+/users/([0-9]+)/profile$"
    replacement: "/internal/profile/$1"
  headers:
    set:
      - name: X-Rewritten-By
        value: "edgion"
```

**Effect**: `/api/v2/users/42/profile` → `/internal/profile/42` (the original
query string is preserved automatically).

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

1. **JSON body operations**, in list order
2. **URI rewrite** (`uri` or `regexUri`)
3. **Host rewrite**
4. **Method rewrite**
5. **Headers modification** (add → set → remove)

---

## Important Notes

### JSON body rewrite

`jsonBody` applies an ordered RFC 6902 JSON Patch transaction. `path` and
`from` use RFC 6901 JSON Pointer syntax; the empty pointer (`""`) identifies the
document root. All operations must succeed before the rewritten body is
published, so a failed later operation never exposes an earlier partial change.
The operator schema is strict: unknown operation members are rejected instead
of being ignored as they would be in a generic JSON Patch document.

```yaml
config:
  jsonBody:
    operations:
      - op: test
        path: /version
        value: 1
      - op: remove
        path: /credentials/password
      - op: replace
        path: /profile/tier
        value: premium
      - op: add
        path: /enabled
        value: true
      - op: copy
        from: /profile
        path: /profileSnapshot
      - op: move
        from: /legacyName
        path: /name
```

`add`, `replace`, and `test` require exactly one of:

- `value`: a literal JSON value, including explicit `null`.
- `valueTemplate`: a shared `${type:name}` template whose rendered result is
  inserted as one JSON string. It is never parsed as a JSON fragment or
  re-expanded as another template. A missing source rejects the request.

Numeric `value` literals follow the Kubernetes/serde_json decoded number
model. If the exact source decimal spelling or precision beyond that model is
part of the application contract, encode it as a JSON string.

For example:

```yaml
jsonBody:
  operations:
    - op: add
      path: /tenant
      valueTemplate: "${header:X-Tenant}"
```

The operation list is limited to 64 entries and a 64 KiB serialized plan.
Pointers are limited to 4 KiB and 64 tokens. Request and output bodies default
to 64 KiB; an entry-level `body.maxBodySize` may raise both limits up to the
fixed 1 MiB ceiling. JSON nesting, node count, and mutation work are also
bounded.

Only one `application/json` or concrete `application/*+json` Content-Type is
accepted. Charset, when present, must be UTF-8, and Content-Encoding must be
absent or `identity`. Invalid JSON, duplicate object keys, failed operations,
failed `test` operations, or missing template values return `400`; unsupported
media returns `415`; input or output size violations return `413`.

Body rewriting preserves parsed JSON meaning, not original whitespace,
object-key order, or a number's original lexical representation. Numbers use
the standard `serde_json` model, and `test` treats common parsed equivalents
such as `1` and `1.0` as equal. Templates observe the request state from before
URI, Host, method, and header rewrites.

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
  namespace: production
spec:
  requestPlugins:
    - type: ProxyRewrite
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
    - type: BasicAuth
      config:
        secretGroups:
          - name: default
            secretRefs:
              - name: api-users
    - type: ProxyRewrite
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
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST,PUT,DELETE"
    - type: ProxyRewrite
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

---

## Related Links

- [ResponseRewrite](./response-rewrite.md) — Response rewriting
- [URL Rewrite](../gateway-api/url-rewrite.md) — Gateway API standard URL rewriting
