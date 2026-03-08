# ForwardAuth Plugin

## Overview

The ForwardAuth plugin forwards key information from the original request (Headers, Method, URI, etc.) to an external authentication service, and decides whether to allow or reject the request based on the authentication service's response status code.

This is the classic external authentication pattern for API Gateways, comparable to:
- **Traefik**: `forwardAuth` middleware
- **nginx**: `auth_request` module
- **APISIX**: `forward-auth` plugin
- **Kong**: `forward-auth` plugin

## Features

- **External authentication delegation** - Fully delegates authentication logic to an external service; the gateway doesn't need to understand authentication details
- **Header forwarding** - Supports both full forwarding and selective forwarding modes
- **Bidirectional header passing** - On success, identity information can be passed to upstream; on failure, error information can be returned to the client
- **Graceful degradation** - When the auth service is unavailable, requests can optionally pass through (degraded mode) or return a custom error code
- **Custom success status codes** - Not limited to 2xx; you can customize which status codes are considered authentication success
- **Connection pool reuse** - Based on a globally shared HTTP Client, reuses connection pools across plugin instances

## Core Flow

```
Client Request
     │
     ▼
ForwardAuth Plugin
     │
     ├─── Build auth request (Header + X-Forwarded-* metadata)
     │
     ├─── Send to external auth service
     │
     ├─── Auth service returns 2xx?
     │       │
     │       ├── Yes → Copy upstreamHeaders to original request → Forward to upstream
     │       │
     │       └── No → Copy clientHeaders + Return auth service's status code and Body
     │
     └─── Auth service unreachable?
             │
             ├── allowDegradation: true → Skip auth, allow through
             │
             └── allowDegradation: false → Return statusOnError (default 503)
```

## Configuration

### Configuration Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `uri` | string | Yes | - | Auth service address (must start with `http://` or `https://`) |
| `requestMethod` | string | No | `GET` | HTTP method sent to the auth service |
| `requestHeaders` | string[] | No | `null` | Request headers to forward to the auth service (see below) |
| `upstreamHeaders` | string[] | No | `[]` | Headers copied from auth response to original request on success |
| `clientHeaders` | string[] | No | `[]` | Headers copied from auth response to client response on failure |
| `timeoutMs` | integer | No | `10000` | Request timeout (milliseconds) |
| `successStatusCodes` | integer[] | No | `null` | Custom success status code list (default: any 2xx) |
| `allowDegradation` | boolean | No | `false` | Whether to allow requests through when auth service is unavailable |
| `statusOnError` | integer | No | `503` | Status code returned on auth service network errors (200-599) |

### requestHeaders Behavior

| Configuration | Behavior |
|--------------|----------|
| Not set (`null`) | Forward **all** request headers (automatically skips hop-by-hop headers) |
| Set to empty array `[]` | Same as above, forward all |
| Set to specific list | Forward **only** the headers specified in the list |

> **Note**: Cookie is a standard HTTP Header (`Cookie: xxx`). In full forwarding mode, it is automatically included.
> In selective mode, add `Cookie` to the `requestHeaders` list to forward it.

### Automatically Added X-Forwarded-* Headers

Regardless of the forwarding mode, the plugin automatically adds the following metadata headers to the auth request:

| Header | Description | Example |
|--------|-------------|---------|
| `X-Forwarded-Host` | Original request's Host | `api.example.com` |
| `X-Forwarded-Uri` | Original request's URI path | `/api/v1/users` |
| `X-Forwarded-Method` | Original request's HTTP method | `POST` |
| `X-Forwarded-Query` | Original request's Query parameters | `page=1&size=20` |

### Hop-by-Hop Header Filtering

The following HTTP hop-by-hop headers are automatically filtered in full forwarding mode (RFC 2616/7230):

`connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`,
`te`, `trailers`, `transfer-encoding`, `upgrade`

## Usage Examples

### Example 1: Basic Configuration - Forward All Headers

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: forward-auth-basic
  namespace: default
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "http://auth-service.auth.svc:8080/verify"
        upstreamHeaders:
          - X-User-ID
          - X-User-Role
          - X-User-Email
```

Forwards all original request headers (skipping hop-by-hop), and on successful authentication copies `X-User-ID`, `X-User-Role`, and `X-User-Email` from the auth response to the upstream request.

### Example 2: Selective Header Forwarding

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: forward-auth-selective
  namespace: default
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "https://auth.example.com/api/verify"
        requestMethod: POST
        timeoutMs: 5000
        requestHeaders:
          - Authorization
          - Cookie
          - X-Request-ID
        upstreamHeaders:
          - X-User-ID
          - X-User-Role
        clientHeaders:
          - WWW-Authenticate
          - X-Auth-Error-Code
        successStatusCodes: [200, 204]
```

Only forwards `Authorization`, `Cookie`, and `X-Request-ID` headers to the auth service.
Uses POST method with a 5-second timeout. Only 200 and 204 are considered authentication success.
On failure, returns `WWW-Authenticate` and `X-Auth-Error-Code` to the client.

### Example 3: Graceful Degradation Mode

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: forward-auth-degraded
  namespace: default
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "http://auth-service:8080/verify"
        allowDegradation: true
        upstreamHeaders:
          - X-User-ID
```

When the auth service is unavailable (network errors, timeouts, etc.), skips authentication and allows requests through.
Suitable for scenarios where the auth service is not on the critical path, prioritizing availability.

### Example 4: Custom Error Status Code

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: forward-auth-custom-error
  namespace: default
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "http://auth-service:8080/verify"
        statusOnError: 403
        upstreamHeaders:
          - X-User-ID
```

Returns 403 instead of the default 503 when the auth service is unreachable.

### Example 5: With HTTPRoute

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: default
spec:
  parentRefs:
    - name: edgion-gateway
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
            name: forward-auth-basic
      backendRefs:
        - name: api-backend
          port: 8080
```

## Comparison with Other Gateways

| Feature | Edgion | Traefik | APISIX | nginx |
|---------|--------|---------|--------|-------|
| Forward all headers | ✅ | ✅ | ❌ (auto headers only) | ✅ (subrequest) |
| Selective header forwarding | ✅ `requestHeaders` | ✅ `authRequestHeaders` | ✅ `request_headers` | ❌ |
| Upstream header passing | ✅ `upstreamHeaders` | ✅ `authResponseHeaders` | ✅ `upstream_headers` | ✅ `auth_request_set` |
| Client header passing | ✅ `clientHeaders` | ❌ | ✅ `client_headers` | Limited (WWW-Authenticate) |
| Custom success status codes | ✅ `successStatusCodes` | ❌ (2xx only) | ❌ (2xx only) | ❌ (2xx only) |
| Graceful degradation | ✅ `allowDegradation` | ❌ | ✅ `allow_degradation` | ❌ |
| Custom error status code | ✅ `statusOnError` | ❌ | ✅ `status_on_error` | ❌ |
| Cookie forwarding | ✅ (full or selective) | ✅ | ✅ | ✅ |
| TLS support | ✅ (rustls) | ✅ | ✅ | ✅ |
| Forward body | ❌ | ✅ `forwardBody` | ❌ | ❌ |
| Regex header matching | ❌ | ✅ `authResponseHeadersRegex` | ❌ | ❌ |

## Auth Service Development Guide

### Interface Contract

The ForwardAuth plugin's interface contract for the auth service is as follows:

**Request**:
- Method: determined by `requestMethod` (default GET)
- Path: determined by `uri`
- Headers: includes original request headers (or a selective subset) + `X-Forwarded-*` metadata

**Response**:
- **2xx** (or status codes in `successStatusCodes`): authentication passed
  - Headers listed in `upstreamHeaders` are copied to the original request
- **Non-2xx**: authentication denied
  - Status code and Body are returned as-is to the client
  - Headers listed in `clientHeaders` are copied to the client response

### Example Auth Service (Go)

```go
func authHandler(w http.ResponseWriter, r *http.Request) {
    token := r.Header.Get("Authorization")
    
    user, err := validateToken(token)
    if err != nil {
        w.Header().Set("WWW-Authenticate", "Bearer")
        w.WriteHeader(http.StatusUnauthorized)
        json.NewEncoder(w).Encode(map[string]string{
            "error": "invalid_token",
            "message": err.Error(),
        })
        return
    }
    
    // Auth passed: pass user identity info via headers
    w.Header().Set("X-User-ID", user.ID)
    w.Header().Set("X-User-Role", user.Role)
    w.Header().Set("X-User-Email", user.Email)
    w.WriteHeader(http.StatusOK)
}
```

## Notes

1. **Connection pool sharing**: All ForwardAuth plugin instances share the same HTTP Client connection pool, reusing TCP connections across instances
2. **No redirect following**: The HTTP Client disables automatic redirect following; 301/302 from the auth service will be treated as authentication failure
3. **Timeout protection**: Default 10-second request timeout; adjust based on the auth service's actual response time
4. **Real-time updates**: After updating the EdgionPlugins resource, configuration is automatically hot-reloaded
5. **Configuration validation**: Empty URI, invalid HTTP method, timeout of 0, etc. will return a 500 error at runtime
6. **Body not forwarded**: The current version does not forward the original request's Body to the auth service (most auth scenarios don't need it)

## FAQ

### Q: Should I use full forwarding or selective forwarding?

A:
- **Full forwarding** (don't set `requestHeaders`): Simple; the auth service can use any original header as needed.
  Suitable for internal auth services and scenarios with lower security requirements.
- **Selective forwarding** (set `requestHeaders`): Only passes necessary headers, reduces data transfer, more secure.
  Suitable for external auth services and scenarios requiring the principle of least privilege.

### Q: How do I forward cookies?

A: Cookie is a standard HTTP Header. In full forwarding mode, it is automatically included; in selective mode, add `Cookie`
to the `requestHeaders` list.

### Q: Is the auth service response body passed to the client?

A: On authentication **failure**, the auth service's response Body is returned as-is to the client (suitable for passing error details).
On authentication **success**, the Body is ignored.

### Q: Can multiple ForwardAuth plugins be chained?

A: Yes. Multiple ForwardAuth plugins execute in the order they appear in the `requestPlugins` array.
Any non-2xx response rejects the request.

### Q: Does `allowDegradation` have security risks?

A: Yes. When enabled, requests pass through without authentication when the auth service is unavailable.
Only suitable for scenarios where authentication is not a critical security barrier (e.g., A/B testing, non-sensitive APIs).
For security-critical APIs, keep the default `false`.
