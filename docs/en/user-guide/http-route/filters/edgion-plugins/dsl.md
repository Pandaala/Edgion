# DSL Plugin

> **🔌 Edgion Extension**
> 
> DSL is a custom scripting plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

The DSL plugin allows custom request processing logic through inline EdgionDSL scripts. Scripts execute in a secure sandbox VM with configurable resource limits. Suitable for scenarios requiring flexible customization without developing standalone plugins.

**Features**:
- Sandboxed execution with configurable resource limits
- Supports both source code and pre-compiled bytecode
- Configurable error policies

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-plugin
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "header-check"
        source: |
          let token = req.header("X-Api-Token")
          if token == nil {
            return deny(403, "missing X-Api-Token header")
          }
```

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `name` | String | Yes | none | Script name (used for logging and debugging) |
| `source` | String | Yes* | none | DSL source code |
| `bytecode` | String | Yes* | none | Pre-compiled bytecode (Base64) |
| `maxSteps` | Integer | No | `10000` | Maximum execution steps |
| `maxLoopIterations` | Integer | No | `100` | Maximum loop iterations |
| `maxCallCount` | Integer | No | `500` | Maximum function call count |
| `maxStackDepth` | Integer | No | `128` | Maximum stack depth |
| `maxStringLen` | Integer | No | `8192` | Maximum string length |
| `errorPolicy` | String | No | `Ignore` | Error policy: `Ignore` / `Deny` / `DenyWith` |

\* `source` and `bytecode` are mutually exclusive; at least one must be provided.

### Error Policies

| Policy | Behavior |
|--------|----------|
| `Ignore` | Ignore script execution errors, continue processing |
| `Deny` | Reject request on script error (returns 500) |
| `DenyWith` | Return custom status code and message on error |

---

## Usage Scenarios

### Scenario 1: Header Validation

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "require-api-token"
      source: |
        let token = req.header("X-Api-Token")
        if token == nil {
          return deny(403, "missing X-Api-Token header")
        }
      errorPolicy: deny
```

### Scenario 2: Conditional Routing Tags

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "set-routing-tag"
      source: |
        let ua = req.header("User-Agent")
        if ua != nil && contains(ua, "Mobile") {
          req.set_header("X-Client-Type", "mobile")
        } else {
          req.set_header("X-Client-Type", "desktop")
        }
```

### Scenario 3: Strict Resource Limits

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "strict-check"
      source: |
        let method = req.method()
        if method == "DELETE" {
          return deny(405, "DELETE not allowed")
        }
      maxSteps: 1000
      maxLoopIterations: 10
      errorPolicy: ignore
```

---

## DSL API Reference

All available APIs are categorized below. APIs marked with **NEW** are newly added.

### Request Read (req.*)

| API | Parameters | Return | Description |
|-----|------------|--------|-------------|
| `req.header(name)` | 1 string | string \| nil | Get request header |
| `req.method()` | 0 | string | HTTP method |
| `req.path()` | 0 | string | Request path |
| `req.query(name)` | 1 string | string \| nil | Get query parameter |
| `req.query_string()` | 0 | string | Full query string |
| `req.cookie(name)` | 1 string | string \| nil | Get cookie value |
| `req.client_ip()` | 0 | string | Client IP |
| `req.remote_ip()` | 0 | string | Remote IP (direct connection) |
| `req.path_param(name)` | 1 string | string \| nil | URL path parameter |
| `req.header_names()` | 0 | list | All request header names (max 256) |
| `req.scheme()` | 0 | string | Request scheme (http/https) |
| `req.host()` | 0 | string | Host header value |
| `req.uri()` | 0 | string | Full URI |
| `req.content_type()` | 0 | string \| nil | Content-Type header |
| `req.has_header(name)` | 1 string | bool | Check if header exists |

### Request Metadata — **NEW**

| API | Parameters | Return | Description |
|-----|------------|--------|-------------|
| `req.protocol` | 0 | string | Protocol type: "http", "grpc", "grpc-web", "websocket" **NEW** |
| `req.grpc_service` | 0 | string \| nil | gRPC service name (from path) **NEW** |
| `req.grpc_method` | 0 | string \| nil | gRPC method name (from path) **NEW** |
| `req.content_length` | 0 | int \| nil | Parsed Content-Length value **NEW** |
| `req.client_cert_cn` | 0 | string \| nil | mTLS client certificate Common Name **NEW** |
| `req.client_cert_fingerprint` | 0 | string \| nil | Client certificate fingerprint hash **NEW** |

### Request Mutation (req.*)

| API | Parameters | Return | Description |
|-----|------------|--------|-------------|
| `req.set_header(name, value)` | 2 strings | nil | Set request header |
| `req.append_header(name, value)` | 2 strings | nil | Append request header |
| `req.remove_header(name)` | 1 string | nil | Remove request header |
| `req.set_uri(uri)` | 1 string | nil | Rewrite request URI |
| `req.set_host(host)` | 1 string | nil | Rewrite Host header |
| `req.set_method(method)` | 1 string | nil | Change HTTP method |

### Response Read (resp.*)

| API | Parameters | Return | Description |
|-----|------------|--------|-------------|
| `resp.status` | 0 | int | Response status code |
| `resp.header(name)` | 1 string | string \| nil | Get response header |
| `resp.has_header(name)` | 1 string | bool | Check response header exists |
| `resp.header_names()` | 0 | list | All response header names (max 256) **NEW** |
| `resp.content_type` | 0 | string \| nil | Response Content-Type **NEW** |

### Response Mutation (resp.*)

| API | Parameters | Return | Description |
|-----|------------|--------|-------------|
| `resp.set_header(name, value)` | 2 strings | nil | Set response header |
| `resp.append_header(name, value)` | 2 strings | nil | Append response header |
| `resp.remove_header(name)` | 1 string | nil | Remove response header |
| `resp.set_status(code)` | 1 int (100-599) | nil | Set response status code **NEW** |

### Context Storage (ctx.*)

| API | Parameters | Return | Description |
|-----|------------|--------|-------------|
| `ctx.get(key)` | 1 string | string \| nil | Get shared context value |
| `ctx.set(key, value)` | 2 strings | nil | Set shared context value |
| `ctx.remove(key)` | 1 string | nil | Remove context key |

### Utility Functions

| Function | Parameters | Return | Description |
|----------|------------|--------|-------------|
| `contains(haystack, needle)` | 2 strings | bool | String contains check |
| `starts_with(str, prefix)` | 2 strings | bool | String prefix check |
| `ends_with(str, suffix)` | 2 strings | bool | String suffix check |
| `len(str)` | 1 string/list | int | String/list length |
| `to_lower(str)` | 1 string | string | Lowercase |
| `to_upper(str)` | 1 string | string | Uppercase |
| `substr(str, start, end)` | 3 | string | Substring |
| `to_string(val)` | 1 any | string | Convert to string |
| `to_int(str)` | 1 string | int \| nil | Parse integer |
| `base64_encode(str)` | 1 string | string | Base64 encode |
| `base64_decode(str)` | 1 string | string | Base64 decode |
| `url_encode(str)` | 1 string | string | URL encode |
| `url_decode(str)` | 1 string | string | URL decode |
| `sha256(str)` | 1 string | string | SHA-256 hex hash |
| `md5(str)` | 1 string | string | MD5 hex hash |
| `time_now()` | 0 | int | Current Unix timestamp |
| `regex_find(str, pattern)` | 2 strings | string \| nil | Regex first match |
| `regex_replace(str, pat, rep)` | 3 strings | string | Regex replace all |
| `trim(str)` | 1 string | string | Trim whitespace |
| `split(str, delimiter)` | 2 strings | list | Split into list |
| `range(start, end)` | 2 ints | list | Integer range |
| `hmac_sha256(key, data)` | 2 strings | string | HMAC-SHA256 hex **NEW** |
| `json_get(json_str, path)` | 2 strings | string \| nil | JSON dot-path extraction **NEW** |
| `uuid()` | 0 | string | Generate UUID v4 **NEW** |
| `hex_encode(str)` | 1 string | string | Hex encode **NEW** |
| `hex_decode(hex)` | 1 string | string | Hex decode **NEW** |
| `time_format(timestamp, fmt)` | int + string | string | Format Unix timestamp **NEW** |
| `ip_in_cidr(ip, cidr)` | 2 strings | bool | Check IP in CIDR range **NEW** |
| `join(list, delimiter)` | list + string | string | Join list items **NEW** |
| `log_warn(msg)` | 1 string | nil | Emit WARN log **NEW** |
| `log_error(msg)` | 1 string | nil | Emit ERROR log **NEW** |

### Return Statements

| Statement | Description |
|-----------|-------------|
| `return deny(status, message)` | Deny request with HTTP status and body |
| `return allow()` | Explicitly allow (no-op) |
| `return next()` | Skip to next plugin |
| `return terminate()` | Terminate TCP connection immediately (no response) **NEW** |
| `return grpc_error(code, message)` | Return native gRPC error via HTTP/2 trailers **NEW** |

---

## gRPC Error Response

The `grpc_error()` return statement provides native gRPC error handling, sending proper gRPC status codes via HTTP/2 trailers.

### Syntax

```dsl
return grpc_error(code, message)
```

- **code**: gRPC status code (integer 0-16). Values outside this range are clamped to 13 (INTERNAL).
- **message**: Human-readable error message string.

### Example: gRPC Authentication Check

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "grpc-auth"
      source: |
        if req.protocol != "grpc" {
          return next()
        }
        let token = req.header("authorization")
        if token == nil {
          return grpc_error(16, "missing authorization token")
        }
      errorPolicy: deny
```

### Phase Restrictions

> **Warning**: `grpc_error()` is **only supported in `requestPlugins`**.

| Plugin Phase | grpc_error() Support | Behavior |
|-------------|---------------------|----------|
| `requestPlugins` | Fully supported | Sends HTTP 200 + grpc-status trailers |
| `upstreamResponseFilterPlugins` | Not supported | Response already committed, degrades to terminate |
| `upstreamResponsePlugins` | Not supported | Response already sent, degrades to terminate |

**Reason**: gRPC errors are carried in HTTP/2 trailers. These can only be set when creating a new response (request phase). Once response headers are committed, trailers cannot be appended.

### gRPC Status Codes

| Code | Name | Description |
|------|------|-------------|
| 0 | OK | No error |
| 1 | CANCELLED | Operation cancelled |
| 2 | UNKNOWN | Unknown error |
| 3 | INVALID_ARGUMENT | Invalid argument |
| 4 | DEADLINE_EXCEEDED | Deadline exceeded |
| 5 | NOT_FOUND | Resource not found |
| 6 | ALREADY_EXISTS | Resource already exists |
| 7 | PERMISSION_DENIED | Permission denied |
| 8 | RESOURCE_EXHAUSTED | Resource exhausted |
| 9 | FAILED_PRECONDITION | Failed precondition |
| 10 | ABORTED | Operation aborted |
| 11 | OUT_OF_RANGE | Out of range |
| 12 | UNIMPLEMENTED | Not implemented |
| 13 | INTERNAL | Internal error |
| 14 | UNAVAILABLE | Service unavailable |
| 15 | DATA_LOSS | Data loss |
| 16 | UNAUTHENTICATED | Not authenticated |

---

## Limitations & Constraints

| Function | Limit | Description |
|----------|-------|-------------|
| `json_get(json, path)` | Path max 32 segments; input/output max 8192 bytes | Returns error if exceeded |
| `time_format(ts, fmt)` | Format string max 256 characters | Returns error if exceeded |
| `regex_find` / `regex_replace` | Regex pattern max 1024 bytes | Returns error if exceeded |
| `req.header_names()` / `resp.header_names()` | Returns max 256 header names | Excess headers truncated |
| `resp.set_status(code)` | Status code range 100-599 | Returns error if out of range |
| All string return values | Subject to `maxStringLen` limit (default 8192) | Returns StringTooLong error |

---

## Important Notes

1. DSL scripts run in a sandbox with no access to the file system or network
2. Scripts exceeding resource limits (`maxSteps`, `maxLoopIterations`, etc.) are terminated
3. Using `bytecode` skips runtime compilation for better performance
4. In production, set `errorPolicy` to `Ignore` or `DenyWith` to prevent script errors from causing service disruptions

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: dsl-route
spec:
  parentRefs:
    - name: my-gateway
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
            name: dsl-validation
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-validation
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "request-validation"
        source: |
          let token = req.header("Authorization")
          if token == nil {
            return deny(401, "Authorization header required")
          }
          let method = req.method()
          if method == "DELETE" {
            let admin = req.header("X-Admin-Token")
            if admin == nil {
              return deny(403, "Admin token required for DELETE")
            }
          }
        errorPolicy: deny
```

## Related Docs

- [Filters Overview](../overview.md)
- [Plugin Composition](../plugin-composition.md)
