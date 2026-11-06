# EdgionDSL Real-World Examples

This document contains complete configuration examples for EdgionDSL in real production scenarios, with a summary of best practices and common anti-patterns at the end. Each example includes a complete `EdgionPlugins` CRD configuration that can be used directly as a reference.

---

## 1. Authentication and Authorization

### 1.1 API Key Verification

The simplest authentication scenario: extract the API Key from the request header, compare it against an expected value, and upon success mark the authentication status in the context for downstream plugins to use.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-key-check
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "api-key-check"
        errorPolicy: deny
        source: |
          let key = req.header("X-API-Key")
          if key == nil {
            return deny(401, "Missing API key")
          }
          if key != "expected-secret-key" {
            return deny(403, "Invalid API key")
          }
          ctx.set("authenticated", "true")
          return next()
```

Notes:

- `deny(401, ...)` is used for "missing credentials" and `deny(403, ...)` for "invalid credentials" — this is semantically more accurate.
- `ctx.set("authenticated", "true")` writes request context for subsequent plugins to read via `ctx.get("authenticated")`. Logging it requires an explicit `AccessLogExtern` declaration with `from: ctx`; see the audit example below.
- `errorPolicy: deny` means that if the script itself errors, the request is also denied, preventing authentication bypass due to script exceptions.

---

### 1.2 JWT Token Extraction and Route Tagging

Extract the Bearer Token from the `Authorization` request header, decode the Base64-encoded Payload segment, use `json_get` to extract claims, and write key fields into the context for downstream use.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwt-claim-extractor
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "jwt-claim-extractor"
        errorPolicy: deny
        source: |
          let auth = req.header("Authorization")
          if auth == nil {
            return deny(401, "Authorization header required")
          }
          if !auth.starts_with("Bearer ") {
            return deny(401, "Authorization must be Bearer token")
          }

          // Extract the token string (strip the "Bearer " prefix, which is 7 characters)
          let token = substr(auth, 7, len(auth))

          // A JWT consists of three base64url segments; take the second one (Payload)
          let parts = split(token, ".")
          let mut payload_b64 = nil
          let mut idx = 0
          for part in parts {
            if idx == 1 {
              payload_b64 = part
              break
            }
            idx = idx + 1
          }

          if payload_b64 == nil {
            return deny(400, "Malformed JWT: missing payload segment")
          }

          // Decode the Payload
          let payload_json = base64_decode(payload_b64)
          if payload_json == nil {
            return deny(400, "Malformed JWT: payload base64 decode failed")
          }

          // Extract common claims
          let sub = json_get(payload_json, "$.sub")
          let role = json_get(payload_json, "$.role")
          let tenant = json_get(payload_json, "$.tenant_id")

          if sub == nil {
            return deny(401, "JWT missing sub claim")
          }

          // Write claims into context and request headers to pass to the upstream
          ctx.set("user_id", sub)
          req.set_header("X-User-Id", sub)

          if role != nil {
            ctx.set("user_role", role)
            req.set_header("X-User-Role", role)
          }

          if tenant != nil {
            ctx.set("tenant_id", tenant)
            req.set_header("X-Tenant-Id", tenant)
          }

          return next()
```

Notes:

- This example only performs structural parsing — **signature verification is not performed**. In production environments, signature validation should be delegated to a dedicated JWT Auth plugin; the DSL is responsible for further claim extraction after verification succeeds.
- `json_get` returns the selected string, integer, boolean, serialized array/object, or `nil`. Check for `nil` before using a required claim.

---

### 1.3 gRPC Authentication Check

Use `req.protocol` to detect whether the current request is gRPC, then validate the `authorization` header. If validation fails, return a standard gRPC status code rather than an HTTP error.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: grpc-auth-check
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "grpc-auth-check"
        errorPolicy: deny
        source: |
          // Non-gRPC requests pass through directly (handled by other plugins)
          if req.protocol != "grpc" {
            return next()
          }

          let token = req.header("authorization")
          if token == nil {
            // gRPC status code 16 = UNAUTHENTICATED
            return grpc_error(16, "missing authorization token")
          }

          if !token.starts_with("Bearer ") {
            return grpc_error(16, "authorization must be Bearer token")
          }

          // Check service-level permissions
          let service = req.grpc_service
          if service == "internal.AdminService" {
            let admin_key = req.header("x-admin-key")
            if admin_key == nil {
              // gRPC status code 7 = PERMISSION_DENIED
              return grpc_error(7, "AdminService requires x-admin-key header")
            }
          }

          return next()
```

Notes:

- gRPC status codes are delivered via HTTP/2 trailers; clients such as tonic can parse them correctly.
- Common status codes: `7` PERMISSION_DENIED, `16` UNAUTHENTICATED.
- `grpc_error()` supports both request and response stages. Request-stage calls send a direct trailers-only response; response-stage calls abort the upstream response and preserve the exact code and optional message for the canonical terminal writer. Wire HTTP status is `200`; `grpc_status` records the separate gRPC application status.

---

## 2. Request Routing and Rewriting

### 2.1 User-Agent-Based Client Classification

Parse the `User-Agent` request header to identify mobile and desktop clients, then inject a normalized `X-Client-Type` header so that upstream services can differentiate behavior by device type.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ua-client-classify
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "ua-client-classify"
        source: |
          let ua = req.header("User-Agent")

          if ua == nil {
            req.set_header("X-Client-Type", "unknown")
            return next()
          }

          // Mobile device keyword detection (earlier entries take priority)
          if ua.contains("Mobile") || ua.contains("Android") || ua.contains("iPhone") {
            req.set_header("X-Client-Type", "mobile")
            ctx.set("client_type", "mobile")
            return next()
          }

          // Tablet
          if ua.contains("iPad") || ua.contains("Tablet") {
            req.set_header("X-Client-Type", "tablet")
            ctx.set("client_type", "tablet")
            return next()
          }

          // Crawlers / bots
          if ua.contains("Bot") || ua.contains("Crawler") || ua.contains("Spider") {
            req.set_header("X-Client-Type", "bot")
            ctx.set("client_type", "bot")
            return next()
          }

          // Default: desktop
          req.set_header("X-Client-Type", "desktop")
          ctx.set("client_type", "desktop")
          return next()
```

---

### 2.2 Path-Based Upstream Selection

Check the request path prefix and write a context variable to enable path-level service routing in conjunction with a dynamic upstream plugin.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: path-based-upstream
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "path-based-upstream"
        source: |
          let path = req.path()

          // /api/v1/* routes to the legacy service
          if path.starts_with("/api/v1/") {
            ctx.set("upstream_cluster", "backend-v1")
            ctx.set("api_version", "v1")
            return next()
          }

          // /api/v2/* routes to the new service
          if path.starts_with("/api/v2/") {
            ctx.set("upstream_cluster", "backend-v2")
            ctx.set("api_version", "v2")
            return next()
          }

          // /internal/* is only accessible from internal networks
          if path.starts_with("/internal/") {
            // Deliberately enforce the physical peer for this network-origin rule.
            let ip = req.direct_peer_ip()
            if !ip_in_cidr(ip, "10.0.0.0/8") {
              return deny(403, "internal endpoints are not publicly accessible")
            }
            ctx.set("upstream_cluster", "backend-internal")
            return next()
          }

          // All other paths route to the default service
          ctx.set("upstream_cluster", "backend-default")
          return next()
```

---

### 2.3 A/B Test Traffic Tagging

Use `hmac_sha256` to deterministically hash the client IP and distribute traffic across experiment groups by percentage. The same client always falls into the same group.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ab-test-marker
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "ab-test-marker"
        source: |
          // Prefer an existing experiment group cookie to avoid recomputing
          let existing = req.cookie("ab_group")
          if existing != nil {
            ctx.set("ab_group", existing)
            req.set_header("X-AB-Group", existing)
            return next()
          }

          let ip = req.remote_ip()

          // Apply HMAC with a fixed salt to get a deterministic hash
          let hash = hmac_sha256("ab-test-salt-2024", ip)

          // Take the last two characters of the hash, convert to an integer, modulo 100 to get a bucket in 0–99
          let hash_len = len(hash)
          let tail = substr(hash, hash_len - 2, hash_len)
          let bucket = hex_to_int(tail)

          // 20% of traffic enters experiment group B; the rest go to control group A
          let mut group = "A"
          if bucket < 51 {
            group = "B"
          }

          ctx.set("ab_group", group)
          req.set_header("X-AB-Group", group)
          return next()
```

Notes:

- The output of `hmac_sha256` is a hexadecimal string that always returns the same result for the same input, ensuring idempotent traffic splitting.
- Short-circuiting via a cookie avoids recomputing on every request and also keeps the experience consistent for users across IP changes.

---

## 3. Security

### 3.1 Internal IP Rate-Limit Exemption

Check whether the client IP belongs to an internal CIDR range. If so, write an exemption flag into the context that downstream rate-limiting plugins can read to skip the rate-limiting logic.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: internal-ip-bypass
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "internal-ip-bypass"
        source: |
          // Deliberately use the physical peer for this internal-origin rule.
          let ip = req.direct_peer_ip()

          // Check common internal network ranges
          if ip_in_cidr(ip, "10.0.0.0/8") {
            ctx.set("rate_limit_bypass", "true")
            ctx.set("network_zone", "internal-10")
            return next()
          }

          if ip_in_cidr(ip, "172.16.0.0/12") {
            ctx.set("rate_limit_bypass", "true")
            ctx.set("network_zone", "internal-172")
            return next()
          }

          if ip_in_cidr(ip, "192.168.0.0/16") {
            ctx.set("rate_limit_bypass", "true")
            ctx.set("network_zone", "internal-192")
            return next()
          }

          // External request: do not set the exemption flag
          ctx.set("rate_limit_bypass", "false")
          ctx.set("network_zone", "external")
          return next()
```

Notes:

- `ip_in_cidr` supports both IPv4 and IPv6 addresses without requiring any additional address-type handling.
- Downstream rate-limiting plugins read the flag via `ctx.get("rate_limit_bypass")` to decide whether to skip rate limiting.

---

### 3.2 Request Body Size Limit

Use the captured body's actual byte length and reject oversized requests before forwarding. This also covers chunked requests whose size is not known from headers.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: body-size-limit
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "body-size-limit"
        source: |
          // Limit the request body to 10 MB (10 * 1024 * 1024 = 10485760)
          let max_bytes = 10485760
          if req.body.byte_len() > max_bytes {
            return deny(413, "Request body is too large")
          }

          return next()
```

Notes:

- `req.body.byte_len()` reports the raw captured byte count and does not convert the body to a string.
- The same script works whether the shared body cache remains in memory or spills to disk.
- The gateway's physical request-body cap is still the authoritative transport limit; a DSL threshold can only be equal or lower.

---

### 3.3 Sensitive Header Stripping

Iterate over all request header names and remove headers that start with `x-internal-`, preventing external clients from bypassing access control by injecting internal routing headers.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: strip-internal-headers
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "strip-internal-headers"
        source: |
          // Collect the header names to remove.
          // Note: you cannot modify a list while iterating over it inside a for loop.
          // Collect first, then remove in a second pass.
          let mut to_remove = ""

          for name in req.header_names() {
            if name.starts_with("x-internal-") {
              // Build a comma-separated list of header names to remove
              if len(to_remove) == 0 {
                to_remove = name
              } else {
                to_remove = to_remove + "," + name
              }
            }
          }

          // Second pass: remove collected headers
          if len(to_remove) > 0 {
            let names = split(to_remove, ",")
            for name in names {
              req.remove_header(name)
            }
          }

          return next()
```

Notes:

- You cannot call `req.remove_header()` inside a `for ... in req.header_names()` loop body, because doing so modifies the underlying collection being iterated. Collect first, then remove.
- If the set of internal headers is known and small (e.g., three or four fixed names), calling `req.remove_header()` individually for each is simpler and requires no loop.

---

## 4. Response Processing

### 4.1 Security Response Header Injection

In the `upstreamResponsePlugins` phase, uniformly inject standard security response headers into every response, reducing the burden of repeated configuration on upstream services.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: security-headers
  namespace: production
spec:
  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: "inject-security-headers"
        source: |
          // Prevent clickjacking
          resp.set_header("X-Frame-Options", "DENY")

          // Prevent MIME sniffing
          resp.set_header("X-Content-Type-Options", "nosniff")

          // XSS protection (modern browsers have this built in, but kept for legacy browser compatibility)
          resp.set_header("X-XSS-Protection", "1; mode=block")

          // Enforce HTTPS for 1 year
          resp.set_header("Strict-Transport-Security", "max-age=31536000; includeSubDomains")

          // Limit Referrer information leakage
          resp.set_header("Referrer-Policy", "strict-origin-when-cross-origin")

          // Basic CSP: allow only same-origin resources (relax as needed)
          resp.set_header("Content-Security-Policy", "default-src 'self'")

          return next()
```

Notes:

- In the response phase, use `resp.set_header()` to write or overwrite response headers; use `resp.append_header()` to append multi-value headers.
- To inject conditionally based on the path or the upstream response status code, read `resp.status` or `req.path()` first and branch accordingly.

---

### 4.2 Cache Control

Set appropriate `Cache-Control` policies for static assets and API responses separately, based on the request path prefix.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cache-control-policy
  namespace: production
spec:
  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: "cache-control-policy"
        source: |
          let path = req.path()
          let status = resp.status

          // Do not cache error responses
          if status >= 400 {
            resp.set_header("Cache-Control", "no-store")
            return next()
          }

          // Static assets: long-term cache, immutable (fingerprinted filenames ensure freshness on update)
          if path.starts_with("/static/") || path.starts_with("/assets/") {
            resp.set_header("Cache-Control", "public, max-age=31536000, immutable")
            return next()
          }

          // Images and fonts
          if path.ends_with(".png") || path.ends_with(".jpg") || path.ends_with(".woff2") {
            resp.set_header("Cache-Control", "public, max-age=86400")
            return next()
          }

          // API endpoints: disable caching entirely
          if path.starts_with("/api/") {
            resp.set_header("Cache-Control", "no-cache, no-store, must-revalidate")
            resp.set_header("Pragma", "no-cache")
            return next()
          }

          // Default: conditional caching
          resp.set_header("Cache-Control", "no-cache")
          return next()
```

---

### 4.3 Debug Header Stripping

Before sending the response to the client, remove internal debug headers returned by upstream services to avoid leaking internal service information.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: strip-debug-headers
  namespace: production
spec:
  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: "strip-debug-headers"
        source: |
          // Remove common internal debug/trace response headers
          resp.remove_header("X-Debug-Info")
          resp.remove_header("X-Internal-Trace-Id")
          resp.remove_header("X-Server-Name")
          resp.remove_header("X-Pod-Name")
          resp.remove_header("X-Node-Name")
          resp.remove_header("X-Upstream-Latency")
          resp.remove_header("Server")

          // Iterate and strip headers with the x-debug-* prefix
          let mut to_remove = ""
          for name in resp.header_names() {
            if name.starts_with("x-debug-") {
              if len(to_remove) == 0 {
                to_remove = name
              } else {
                to_remove = to_remove + "," + name
              }
            }
          }

          if len(to_remove) > 0 {
            let names = split(to_remove, ",")
            for name in names {
              resp.remove_header(name)
            }
          }

          return next()
```

---

## 5. Observability

### 5.1 Request Audit Logging

Use `ctx.set()` to store request metadata, then explicitly select its access-log fields with `EdgionPlugins.spec.accessLogExtern`. The declaration is read at terminal logging and writes fields under `ext`; it does not execute as a request plugin.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: request-audit-log
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "request-audit-log"
        source: |
          let method = req.method()
          let path = req.path()
          let ip = req.remote_ip()
          // Store values for the explicitly declared access-log fields
          ctx.set("audit_method", method)
          ctx.set("audit_path", path)
          ctx.set("audit_ip", ip)

          // Record authentication presence (desensitized: only log whether it exists)
          let auth = req.header("Authorization")
          if auth != nil {
            ctx.set("has_auth", "true")
          } else {
            ctx.set("has_auth", "false")
          }

          // Record the first 64 characters of User-Agent
          let ua = req.header("User-Agent")
          if ua != nil {
            let ua_short = substr(ua, 0, 64)
            ctx.set("user_agent_short", ua_short)
          }

          return next()
  accessLogExtern:
    - key: audit_method
      from: ctx
      name: audit_method
    - key: audit_path
      from: ctx
      name: audit_path
    - key: audit_ip
      from: ctx
      name: audit_ip
    - key: has_auth
      from: ctx
      name: has_auth
    - key: user_agent_short
      from: ctx
      name: user_agent_short
```

Notes:

- Attach this EdgionPlugins resource to the route as usual. Only the declared, present context keys are projected into `ext`; other `ctx.set()` values remain request-local.
- All `ctx` keys are masked by default (`***` in the default `replace` mode). Readable fields require an explicit, reviewed `EdgionGatewayConfig.spec.accessLogExtern.unmaskedKeys.ctx` entry; the sample does not disable masking.
- `jwt_claims`, `oidc_claims`, and `jwe_payload` cannot be projected, even with an unmasked-key entry or a different sensitive-output mode. Do not copy credentials or full claims to another key or use `log()` to bypass this boundary.
- See [AccessLogExtern](../../../../../ops-guide/observability/access-log.md) for attachment, masking, and blocklist details. The example records authentication presence, never the credential value.

---

### 5.2 Request Latency Tagging

Record a timestamp in the request phase, then calculate the elapsed time in the response phase and write it to a response header, making latency analysis straightforward for clients and monitoring systems.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: latency-marker
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "latency-marker-req"
        source: |
          // Record the request arrival time (Unix milliseconds)
          let start_ms = time_now()
          ctx.set("req_start_ms", to_str(start_ms))

          // Generate a request ID for log correlation
          let req_id = uuid()
          ctx.set("request_id", req_id)
          req.set_header("X-Request-Id", req_id)

          return next()

  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: "latency-marker-resp"
        source: |
          let end_ms = time_now()
          let start_str = ctx.get("req_start_ms")

          if start_str != nil {
            let start_ms = to_int(start_str)
            if start_ms != nil {
              let elapsed_ms = end_ms - start_ms
              resp.set_header("X-Upstream-Latency-Ms", to_str(elapsed_ms))
              ctx.set("upstream_latency_ms", to_str(elapsed_ms))
            }
          }

          // Forward the request ID to the response for client-side correlation
          let req_id = ctx.get("request_id")
          if req_id != nil {
            resp.set_header("X-Request-Id", req_id)
          }

          return next()
  accessLogExtern:
    - key: upstream_latency_ms
      from: ctx
      name: upstream_latency_ms
```

Notes:

- `time_now()` returns a Unix millisecond timestamp (type `Int`).
- Context variables can only store strings, so `to_str()` and `to_int()` are needed to convert numeric values back and forth.
- `ctx.set("upstream_latency_ms", ...)` stores the value in request context. The explicit `from: ctx` declaration below the script selects it for `ext`; it remains masked by default. Numeric aggregation requires a reviewed unmasked-key configuration as described in [AccessLogExtern](../../../../../ops-guide/observability/access-log.md).

---

## 6. Best Practices

### 6.1 Return Early

Pre-conditions such as authentication checks and required field validation should be performed at the start of the script. On failure, return `deny()` immediately to avoid executing unnecessary downstream logic.

```yaml
# Recommended: check prerequisites first, return immediately if not met
source: |
  let key = req.header("X-API-Key")
  if key == nil {
    return deny(401, "missing api key")
  }
  // Subsequent logic is guaranteed: key is not nil
  if key != "valid-key" {
    return deny(403, "invalid api key")
  }
  ctx.set("authenticated", "true")
  return next()
```

### 6.2 Make Good Use of Context Variables

Use context variables to share values within one request. They are not automatically logged. Select only the fields you need through `AccessLogExtern from: ctx`, and retain its masking and categorical blocklist. Use consistent names such as lowercase with underscores.

```yaml
source: |
  ctx.set("user_id", sub)          // Request context only; logging needs an explicit declaration
  ctx.set("tenant_id", tenant)     // Request context only; logging needs an explicit declaration
  ctx.set("ab_group", group)       // Request context only; logging needs an explicit declaration
  return next()
```

### 6.3 Choosing an Error Policy

| Scenario | Recommended Policy | Reason |
|----------|--------------------|--------|
| General traffic processing (header injection, log tagging) | `Ignore` | Script errors should not affect normal requests |
| Authentication / authorization checks | `Deny` | Script errors should fail closed (fail-closed) |
| Compliance auditing / mandatory recording | `DenyWith` | Return a custom error response with a clear message to the client |

```yaml
# Security-sensitive scenario: script errors should deny the request
config:
  name: "auth-check"
  errorPolicy: deny
  source: |
    ...

# Enhancement scenario: script errors should pass the request through
config:
  name: "header-enrichment"
  errorPolicy: Ignore
  source: |
    ...
```

### 6.4 Resource Limit Tuning Principles

The default limits are suitable for the vast majority of scenarios. Do not raise them blindly. Only consider adjustments in the following situations:

- **Lower** `maxLoopIterations` (e.g., to `20`): when the length of the list being iterated is known to be small, a lower value catches anomalies earlier.
- **Raise** `maxStringLen` (e.g., to `32768`): when you need to return longer strings from `read()`, `json_get()`, or process longer Base64-encoded content. Streaming body predicates do not require a complete string allocation.
- Do not increase `maxSteps` to "fix" an infinite loop — inspect the script logic instead.

### 6.5 Bytecode Mode

For DSL plugins on high-traffic routes, consider using precompiled bytecode to skip the compilation overhead incurred on every configuration load.

```yaml
config:
  name: "hot-path-check"
  # Use precompiled bytecode instead of source
  bytecode: "BASE64_ENCODED_BYTECODE_HERE"
```

For instructions on compiling bytecode, see the [Bytecode Guide](./bytecode.md).

### 6.6 Avoid Complex Logic

DSL is suited for lightweight, decision-oriented processing logic (authentication pre-checks, header injection, route tagging). Complex business logic (database queries, remote calls, complex data transformations) should be implemented as dedicated native Rust plugins:

- Native plugins have access to the async runtime and can make real network requests.
- Native plugins are not subject to the sandbox step-count and loop-iteration limits.
- Debugging DSL scripts carries a higher cost than working with type-safe Rust code.

---

## 7. Common Anti-Patterns

### Anti-Pattern 1: Calling Many Functions Inside a Loop

```yaml
# Wrong: calling multiple functions per loop iteration makes it easy to hit the maxCallCount limit
source: |
  for name in req.header_names() {
    let val = req.header(name)           # function call +1
    let lower = to_lower(val)            # function call +1
    let encoded = base64_encode(lower)   # function call +1
    log("header: " + name + "=" + encoded)  # function call +1
  }
  return next()
```

```yaml
# Correct: only call functions when necessary, reducing the number of calls per iteration
source: |
  for name in req.header_names() {
    if name.starts_with("x-log-") {
      let val = req.header(name)
      log("header: " + name + "=" + val)
    }
  }
  return next()
```

### Anti-Pattern 2: Excessive String Concatenation

```yaml
# Wrong: concatenating all request headers may produce a string that exceeds the maxStringLen limit
source: |
  let mut all_headers = ""
  for name in req.header_names() {
    let val = req.header(name)
    all_headers = all_headers + name + ": " + val + "\n"
  }
  log(all_headers)  # may trigger a maxStringLen error
  return next()
```

```yaml
# Correct: only log key headers, and log them individually rather than concatenating
source: |
  let auth = req.header("Authorization")
  let ct = req.header("Content-Type")
  if auth != nil {
    log("auth header present")
  }
  if ct != nil {
    log("content-type: " + ct)
  }
  return next()
```

### Anti-Pattern 3: Not Checking for nil Values

```yaml
# Wrong: calling a method directly on a value that may be nil
source: |
  let ua = req.header("User-Agent")
  # ua may be nil; nil.starts_with() returns false rather than crashing,
  # but string concatenation below converts nil to the literal string "nil"
  log("ua: " + ua)  # when ua is nil, outputs "ua: nil" — not the intended behavior
  return next()
```

```yaml
# Correct: check for nil before use
source: |
  let ua = req.header("User-Agent")
  if ua != nil {
    log("ua: " + ua)
    if ua.starts_with("Mozilla") {
      req.set_header("X-Browser-Client", "true")
    }
  }
  return next()
```

## Response-stage gRPC Error

Abort a failed upstream gRPC response with the exact `RESOURCE_EXHAUSTED` code (`8`) and message. The client receives HTTP `200` and gRPC status `8`; the access log records `grpc_status: 8`.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: grpc-response-error
  namespace: production
spec:
  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: grpc-response-error
        source: |
          if req.protocol == "grpc" && resp.status >= 500 {
            return grpc_error(8, "upstream capacity exhausted")
          }
          return next()
```

---

## Related Documentation

- [DSL Plugin Overview and Configuration Parameters](./README.md)
- [EdgionDSL Language Syntax Reference](./language-syntax.md)
- [API Reference Manual](./api-reference.md)
- [Sandbox Security Model](./sandbox-security.md)
- [Bytecode Guide](./bytecode.md)
