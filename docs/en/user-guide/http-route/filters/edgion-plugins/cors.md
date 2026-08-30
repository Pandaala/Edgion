# CORS Plugin

> **🔌 Edgion extension**
> 
> CORS is a cross-origin resource sharing configuration plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## What is CORS?

**CORS (Cross-Origin Resource Sharing)** is a browser security mechanism that controls whether a web page from one origin can access resources from a different origin.

An **origin** is the combination of protocol + domain + port. For example, `https://app.example.com` and `https://api.example.com` are different origins (different subdomains).

When a frontend application calls an API on a different origin, the browser sends a CORS preflight or attaches an `Origin` header, and the server must respond with the appropriate CORS headers to authorize the access.

## Quick Start

### Simplest configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-plugin
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST,PUT,DELETE"
        allowHeaders: "Content-Type,Authorization"
```

### Default-value notes

The defaults are security-first:

| Field | Default | Meaning |
|-------|---------|---------|
| `allowOrigins` | `""` (empty) | **Rejects all origins** — you must configure it explicitly |
| `allowMethods` | `GET,HEAD,OPTIONS` | Only safe methods |
| `allowHeaders` | `Accept,Accept-Language,Content-Language,Content-Type,Range` | CORS-safelisted request headers |
| `allowCredentials` | `false` | Credentials not allowed |
| `maxAge` | none | No preflight caching |

---

## Configuration parameters

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allowOrigins` | String | `""` (empty) | Comma-separated allowed origins. Supports `*`, `**`, and `*.domain`. Example: `"https://app.example.com,https://admin.example.com"` |
| `allowMethods` | String | `GET,HEAD,OPTIONS` | Comma-separated allowed HTTP methods. Supports `*` / `**`. Emitted as `Access-Control-Allow-Methods` on preflight responses only |
| `allowHeaders` | String | `Accept,Accept-Language,Content-Language,Content-Type,Range` | Comma-separated allowed request headers. Supports `*` / `**`. Emitted as `Access-Control-Allow-Headers` on preflight responses only |
| `exposeHeaders` | String | `""` (empty) | Comma-separated response headers exposed to the browser |
| `allowCredentials` | Boolean | `false` | Whether to allow credentials (cookies, authorization headers) |
| `maxAge` | Integer | none | Preflight cache duration in seconds. When unset, no `Access-Control-Max-Age` header is sent |

### Advanced parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allowOriginsByRegex` | Array\<String\> | none | Regex patterns for origin matching. Use this to allow `http://` origins (the `*.domain` shorthand is HTTPS-only) |
| `preflightContinue` | Boolean | `false` | Forward the OPTIONS preflight request to upstream instead of answering it at the gateway |
| `allowPrivateNetwork` | Boolean | `false` | Enable Private Network Access support (Chrome 94+) |
| `timingAllowOrigins` | String | none | `Timing-Allow-Origin` header value. Supports `*` / `**` |
| `timingAllowOriginsByRegex` | Array\<String\> | none | Regex patterns for `Timing-Allow-Origin` matching |

### Special-value notes

The `allowOrigins` value supports several magic forms:

| Value | Meaning | Use case |
|-------|---------|----------|
| `*` | Wildcard, allows all origins | **Not recommended in production** |
| `**` | Force wildcard, allows all origins | **Avoid** |
| `*.example.com` | Subdomain wildcard, matches `https://<any>.example.com` (HTTPS only; does **not** match the root domain or `http://`) | Multi-subdomain apps |
| `""` (empty) | Reject all origins | Default, safe |

⚠️ **Wildcard origins (`*` / `**`) combined with `allowCredentials: true` are rejected at parse time.** The same restriction applies to `allowMethods`, `allowHeaders`, `exposeHeaders`, and `timingAllowOrigins`: none of them may contain `*` when `allowCredentials` is `true`. This mirrors the browser rule that a credentialed response cannot use a wildcard.

The `null` origin is always rejected, even when `allowOrigins` is `*`.

---

## Common configuration scenarios

### 1. Single-origin frontend

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-single
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST"
        allowHeaders: "Content-Type"
```

### 2. Multiple specified origins with credentials

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-multi
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com,https://admin.example.com"
        allowMethods: "GET,POST,PUT,DELETE"
        allowHeaders: "Content-Type,Authorization"
        allowCredentials: true
```

### 3. Subdomain wildcard

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-subdomain
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        # Matches https://<any>.example.com over HTTPS only
        allowOrigins: "*.example.com"
        allowMethods: "GET,POST"
        allowCredentials: true
```

### 4. Public API with preflight caching (no credentials)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-public
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "*"
        allowMethods: "GET"
        allowHeaders: "Content-Type"
        # Cache the preflight result for 1 hour
        maxAge: 3600
```

### 5. Exposed and reflected headers

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-headers
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST"
        # Reflect whatever request headers the browser asks for
        allowHeaders: "**"
        # Let JavaScript read these response headers
        exposeHeaders: "X-Total-Count,X-Page-Number"
```

### 6. Full production configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-full
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com,https://admin.example.com"
        allowMethods: "GET,POST,PUT,DELETE,PATCH"
        allowHeaders: "Content-Type,Authorization,X-Request-ID"
        exposeHeaders: "X-Total-Count,X-Page-Number"
        allowCredentials: true
        # Cache preflight for 24 hours
        maxAge: 86400
        allowPrivateNetwork: false
```

---

## FAQ

### Q1: Why are my CORS requests still blocked?

**A**: Check these in order:

1. Is `allowOrigins` configured? The default is empty (rejects all).
2. Does the request `Origin` match exactly (protocol + domain + port)?
3. For credentialed requests, is `allowCredentials: true` set?
4. Are the required headers listed in `allowHeaders`?

### Q2: Why does a wildcard origin with credentials fail to apply?

**A**: This is by design. When `allowCredentials: true`, you cannot use `*` or `**` in `allowOrigins` (or in `allowMethods`, `allowHeaders`, `exposeHeaders`, `timingAllowOrigins`), and `allowOriginsByRegex` / `timingAllowOriginsByRegex` are likewise disallowed (a broad pattern such as `.*` would echo any origin back with credentials enabled). The plugin rejects such a config. Use explicit origins or a `*.domain` subdomain wildcard instead.

### Q3: How do I allow `http://` origins for local development?

**A**: The `*.domain` shorthand is HTTPS-only. For `http://` origins, use `allowOriginsByRegex`:

```yaml
config:
  allowOriginsByRegex:
    - "^http://localhost:[0-9]+$"
```

### Q4: Does `*.example.com` match the root domain?

**A**: No. `*.example.com` matches `https://api.example.com` but not `https://example.com`. List the root domain explicitly if you need it: `allowOrigins: "https://example.com,*.example.com"`.

---

## Security best practices

### ✅ Recommended

```yaml
✅ Correct:
# Explicit origins
allowOrigins: "https://app.example.com"
# Specific methods
allowMethods: "GET,POST"
# Specific headers
allowHeaders: "Content-Type,Authorization"
# Enable credentials only when needed
allowCredentials: true
```

### ❌ To avoid

```yaml
❌ Incorrect:
# Wildcard origin in production
allowOrigins: "*"
# Wildcard combined with credentials (rejected at parse time)
allowOrigins: "*"
allowCredentials: true
```

---

## Complete configuration example

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
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: cors-full
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-full
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com,https://admin.example.com"
        allowMethods: "GET,POST,PUT,DELETE,PATCH"
        allowHeaders: "Content-Type,Authorization,X-Request-ID"
        exposeHeaders: "X-Total-Count,X-Page-Number"
        allowCredentials: true
        maxAge: 86400
        allowPrivateNetwork: false
```

## Related links

- [CSRF Plugin](./csrf.md)
- [Filters overview](../README.md)
- [MDN: CORS](https://developer.mozilla.org/en-US/docs/Web/HTTP/CORS)
- [W3C: Private Network Access](https://wicg.github.io/private-network-access/)
