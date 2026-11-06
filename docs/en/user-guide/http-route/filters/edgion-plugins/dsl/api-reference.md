# EdgionDSL API Reference

This document lists all built-in functions in EdgionDSL, organized by category. Each function includes its full signature, parameter descriptions, return values, constraints, and usage examples.

**Type overview**: EdgionDSL has 8 runtime types: `Str` (string), `Int` (64-bit integer), `Bool` (boolean), `Nil` (null/missing), `Body` (an opaque request-body handle), `List` (a list of strings; cannot be constructed directly by user code), and `Map` (an ordered string-keyed map; returned by `http.call()` and constructible via `{"key": expr}` literals).

`Xml` is the eighth type: an opaque immutable XML node, accessed through `xml.*` primitives.

**Phase overview**: Some functions are only valid in specific execution phases. `req.*` write operations affect the request phase; `resp.*` read and write operations apply to the response phase. `grpc_error()` is supported in both phases.

---

## Table of Contents

- [Request Reading (req.*)](#request-reading-req)
- [Request Metadata (req.*)](#request-metadata-req)
- [Request Modification (req.*)](#request-modification-req)
- [Response Reading (resp.*)](#response-reading-resp)
- [Response Modification (resp.*)](#response-modification-resp)
- [Context Storage (ctx.*)](#context-storage-ctx)
- [String Handling](#string-handling)
- [Encoding and Decoding](#encoding-and-decoding)
- [Hashing and Cryptography](#hashing-and-cryptography)
- [Regular Expressions](#regular-expressions)
- [Time](#time)
- [JSON](#json)
- [Network](#network)
- [Redis (redis.*)](#redis-redis)
- [Control Flow Functions](#control-flow-functions)
- [Other Utilities](#other-utilities)

---

## Request Reading (req.*)

### req.header

**Signature**: `req.header(name)` → `Str | Nil`

Reads the value of the specified header from the request. Header names are case-insensitive. Returns `nil` if the header does not exist. If multiple headers with the same name exist, returns the first value.

**Parameters**:
- `name` (Str) — Header name, case-insensitive

**Return value**: The header value as a string, or `nil` if the header is absent

**Example**:
```
let token = req.header("Authorization")
if token == nil {
  return deny(401, "missing Authorization")
}
let bearer = substr(token, 7, len(token))
```

---

### req.method

**Signature**: `req.method` → `Str`

Returns the HTTP method of the current request, preserving its case, including changes made by `req.set_method`. Method names are case-sensitive: `"POST"`, `"post"`, and `"Post"` are distinct. Use the uppercase spelling for standard methods such as `"GET"` and `"POST"`.

**Return value**: HTTP method string

**Example**:
```
if req.method != "GET" and req.method != "POST" {
  return deny(405, "method not allowed")
}
```

---

### req.path

**Signature**: `req.path` → `Str`

Returns the path portion of the request (excluding the query string), always starting with `/`. For example, a request to `/api/v1/users?page=1` returns `"/api/v1/users"`.

**Return value**: Request path string

**Example**:
```
let path = req.path
if path == "/health" {
  return deny(200, "ok")
}
```

---

### req.query

**Signature**: `req.query(name)` → `Str | Nil`

Reads the value of the specified URL query parameter. Returns `nil` if the parameter does not exist. If the same parameter appears multiple times, returns the first value.

**Parameters**:
- `name` (Str) — Query parameter name, case-sensitive

**Return value**: Parameter value string, or `nil`

**Example**:
```
let page = req.query("page")
if page == nil {
  page = "1"
}
```

---

### req.query_string

**Signature**: `req.query_string` → `Str | Nil`

Returns the full query string without the leading `?`. Returns `nil` if the URL has no query string.

**Return value**: Full query string, or `nil`

**Example**:
```
let qs = req.query_string
if qs != nil {
  req.set_header("X-Query-String", qs)
}
```

---

### req.cookie

**Signature**: `req.cookie(name)` → `Str | Nil`

Reads the value of the specified cookie from the request's `Cookie` header. Returns `nil` if the cookie does not exist.

**Parameters**:
- `name` (Str) — Cookie name, case-sensitive

**Return value**: Cookie value string, or `nil`

**Example**:
```
let session = req.cookie("session_id")
if session == nil {
  return deny(401, "session required")
}
req.set_header("X-Session-Id", session)
```

---

### req.direct_peer_ip

**Signature**: `req.direct_peer_ip` → `Str`

Returns the physical downstream transport peer (`DirectPeerIp`). It is not changed by inbound PROXY protocol or forwarded-header processing. Use `req.remote_ip` for ordinary client identity, rate-limit, and experiment-bucketing use cases.

**Return value**: IP address string (IPv4 or IPv6 format)

**Example**:
```
let ip = req.direct_peer_ip
if ip_in_cidr(ip, "10.0.0.0/8") {
  req.set_header("X-Internal-Request", "true")
}
```

---

### req.remote_ip

**Signature**: `req.remote_ip` → `Str`

Returns the resolved client identity (`RemoteIp`) for the current request layer. It starts from the effective L4 peer and may be updated by the trusted-proxy `RealIp`/XFF resolver.

**Return value**: IP address string (IPv4 or IPv6 format)

**Example**:
```
let remote = req.remote_ip
log("resolved client: " + remote)
```

---

### req.path_param

**Signature**: `req.path_param(name)` → `Str | Nil`

Reads the value of a named parameter from the route path template. For example, if the route is `/users/{userId}` and the request path is `/users/42`, then `req.path_param("userId")` returns `"42"`. Returns `nil` if the parameter does not exist.

**Parameters**:
- `name` (Str) — Path parameter name

**Return value**: Path parameter value string, or `nil`

**Example**:
```
let user_id = req.path_param("userId")
if user_id == nil {
  return deny(400, "missing path parameter: userId")
}
req.set_header("X-User-Id", user_id)
```

---

### req.header_names

**Signature**: `req.header_names` → `List`

Returns a list of all header names present in the current request. At most 256 names are returned; any beyond that are truncated. List elements are strings in the order they appear in the request.

**Return value**: A `List` containing all request header names

**Constraints**: At most 256 header names are returned

**Example**:
```
let names = req.header_names
for name in names {
  log("header: " + name)
}
```

---

### req.scheme

**Signature**: `req.scheme` → `Str`

Returns the protocol scheme of the request. Reads from the `X-Forwarded-Proto` header first, then `X-Scheme`, and falls back to `"http"` if neither is present.

**Return value**: Protocol scheme string, typically `"http"` or `"https"`

**Example**:
```
if req.scheme != "https" {
  return deny(400, "HTTPS required")
}
```

---

### req.host

**Signature**: `req.host` → `Str | Nil`

Returns the value of the `Host` header from the request. Returns `nil` if the request does not carry a `Host` header.

**Return value**: Host string (may include port, e.g. `"example.com:8080"`), or `nil`

**Example**:
```
let host = req.host
if host == nil or host != "api.example.com" {
  return deny(403, "invalid host")
}
```

---

### req.uri

**Signature**: `req.uri` → `Str`

Returns the full request URI, including the path and query string. If a query string is present, the format is `path?query`; otherwise only the path is returned.

**Return value**: Full URI string

**Example**:
```
let uri = req.uri
ctx.set("original_uri", uri)
req.set_uri("/api/v2" + req.path)
```

---

### req.content_type

**Signature**: `req.content_type` → `Str | Nil`

Reads the `Content-Type` header value of the request. Returns `nil` if it does not exist. Equivalent to `req.header("Content-Type")` but with clearer intent.

**Return value**: Content-Type string, or `nil`

**Example**:
```
let ct = req.content_type
if ct != nil and regex_find(ct, "application/json") == nil {
  return deny(415, "only application/json is accepted")
}
```

---

### req.has_header

**Signature**: `req.has_header(name)` → `Bool`

Checks whether the request contains the specified header. Does not return the value — only checks existence. Header names are case-insensitive.

**Parameters**:
- `name` (Str) — Header name

**Return value**: `true` if the header exists, `false` if it does not

**Example**:
```
if not req.has_header("X-Request-Id") {
  req.set_header("X-Request-Id", uuid())
}
```

---

### req.body

**Signature**: `req.body` → `Body`

Returns an opaque handle for the proxied request body. The handle contains no
bytes, cursor, file descriptor, or storage path. Evaluating it checks body
availability but does not read the body into a contiguous string. Every
operation sees the current logical body version, whether it is backed by
memory, a spill file, or mutation segments.

Shared text operations decode incrementally with the same result as
`String::from_utf8_lossy`: invalid UTF-8 is represented by U+FFFD and incomplete
code points are carried across storage chunks. Raw byte reads, hashing, and
mutations still operate on the original bytes.

**Shared `Str` / `Body` text methods**:

| Method | Return | Description |
|--------|--------|-------------|
| `.len()` | `Int` | Unicode scalar count |
| `.is_empty()` | `Bool` | Whether the logical text is empty |
| `.starts_with(s)` | `Bool` | Prefix test |
| `.ends_with(s)` | `Bool` | Suffix test |
| `.contains(s)` | `Bool` | Substring test |
| `.matches("regex")` | `Bool` | Compile-time streaming-regex test |

**Body-only reads**:

| Method | Return | Description |
|--------|--------|-------------|
| `.byte_len()` | `Int` | Raw byte length; metadata-only |
| `.read(offset, max_bytes)` | `Str` | Select the raw range, then lossily decode that isolated slice |
| `.sha256()` | `Str` | 64-character lowercase SHA-256 of the raw bytes; cached per immutable Body generation |
| `.md5()` | `Str` | 32-character lowercase legacy MD5 checksum of the raw bytes |
| `.hmac_sha256(key)` | `Str` | Generate a 64-character lowercase HMAC-SHA-256 over the raw bytes |
| `.json_get("$.a[0]")` | value or `Nil` | Select a compile-time definite JSONPath |
| `.json_select({"name": "$.a"})` | `Map` | Select several named compile-time paths in one parse |

`read(offset, max_bytes)` requires two non-negative integer arguments. The
requested `max_bytes` must not exceed `maxStringLen`: 8,192 bytes (8 KiB) by
default, configurable from 1 to 1,048,576 bytes (1 MiB). An oversized request
raises `StringTooLong` before scan reservation or range clipping, even if the
body is shorter; it is never silently clamped. Within the admitted limit, a
range extending beyond EOF returns the available suffix, and an offset beyond
the body is an error.

The returned text and serialized JSON results must also fit `maxStringLen`,
measured in UTF-8 output bytes. Lossy decoding may produce more output bytes
than the selected raw range. `read` enforces the output limit while decoding, including replacement
characters for invalid or incomplete UTF-8. Overflow raises `StringTooLong`
without returning a truncated prefix and follows the configured `errorPolicy`.
An early-overflow length diagnostic may report only `maxStringLen + 1`, rather
than decoding the rest solely to calculate its complete size.

Copying a Body handle preserves access to the current logical body: after a
successful mutation, all aliases see the new version on their next operation.
An outbound `http.call` already using the Body keeps its call-time snapshot.

Body digest and MAC methods scan the already captured raw bytes; they do not
hash concurrently with client upload and never convert the Body to String.
`sha256()` caches one 32-byte result per immutable Body generation. `md5()` and
`hmac_sha256(key)` scan on every call and consume actual Body scan bytes. MD5
is compatibility-only and must not be used for authentication, signatures,
password storage, or collision-sensitive security. Its availability in a FIPS
build does not make MD5 an approved security algorithm.

`Body.hmac_sha256(key)` accepts an exact `Str`, adds no persistent key/MAC
cache, and is only a MAC-generation API. Ordinary DSL `==` is not
constant-time; do not use it to verify an attacker-supplied authentication
tag. Use a dedicated authentication plugin or external verifier for inbound
signature verification.

**Request-stage-only mutations**:

| Method | Effect |
|--------|--------|
| `.clear()` | Replace the body with empty content |
| `.append(s)` | Append the UTF-8 bytes of exact `Str` argument `s` |
| `.prepend(s)` | Prepend the UTF-8 bytes of exact `Str` argument `s` |
| `.remove(start, end)` | Remove the half-open raw-byte range |
| `.replace(from, to)` | Replace all non-overlapping, leftmost literal byte matches; both arguments must be `Str` |

Mutations return `nil`, publish atomically, remain bounded by the gateway
physical request-body cap, and retain at most 4,096 logical segments. Empty
search patterns and invalid ranges are rejected. Mutation outside
`requestPlugins` is rejected during configuration validation.

**Availability and failure behavior**:

- A request-stage script that statically references `req.body` uses the one
  shared bounded request-body capture. A script without such a reference pays
  no capture cost.
- An `upstreamResponsePlugins` script may read a cache populated earlier
  by `RequestBodyBuffer`; it never starts late capture.
- A known bodyless request uses an empty Body without allocating a cache.
- The pre-VM body gate always fails closed: request-body limit failures return
  413, capture timeouts return 408, and refusals or other capture failures
  return 500. These gate responses have an empty body.
- After the VM starts, missing late capture for a declared or unknown body,
  storage I/O, scan-budget exhaustion, segment overflow, and mutation
  publication failures always return HTTP 500 with the generic response
  `Request body processing failed`, regardless of `errorPolicy`.
- `Body` is truthy but cannot be converted implicitly or by `to_str`. It cannot
  be concatenated, logged, interpolated, used as a map key, passed as a header
  value, or forwarded to another string API. Use an explicit bounded read.

Body/String equality scans logical text in either operand order. Two Body
values compare equal because both identify the current request body. Ordering a
Body is a type error.

**Example**:

```
let body = req.body
if !body.is_empty() and body.contains("token") {
  req.set_header("X-Body-Preview", body.read(0, 128))
  req.set_header("X-Body-SHA256", body.sha256())
}
```

---

## Request Metadata (req.*)

### req.protocol

**Signature**: `req.protocol` → `Str`

Returns the protocol type identifier of the current request. Possible values are:
- `"http"` — Ordinary HTTP/1.1 or HTTP/2 request
- `"grpc"` — gRPC request (`Content-Type: application/grpc`, matched case-insensitively)
- `"grpc-web"` — gRPC-Web request
- `"websocket"` — WebSocket upgrade request

**Return value**: Protocol type string

**Example**:
```
let proto = req.protocol
if proto == "grpc" {
  let svc = req.grpc_service
  req.set_header("X-Grpc-Service", svc)
}
```

---

### req.grpc_service

**Signature**: `req.grpc_service` → `Str | Nil`

Parses the service name from the gRPC request path. The gRPC path format is `/{package}.{ServiceName}/{MethodName}`; this function returns the fully qualified service name including the package name, for example `"example.helloworld.Greeter"`. Returns `nil` if the request is not a gRPC protocol or the path format does not match.

**Return value**: gRPC service name string, or `nil`

**Example**:
```
let svc = req.grpc_service
if svc != nil and svc != "example.auth.AuthService" {
  return grpc_error(12, "service not found")
}
```

---

### req.grpc_method

**Signature**: `req.grpc_method` → `Str | Nil`

Parses the method name from the gRPC request path. For example, if the path is `/helloworld.Greeter/SayHello`, returns `"SayHello"`. Returns `nil` if the request is not a gRPC protocol or the path format does not match.

**Return value**: gRPC method name string, or `nil`

**Example**:
```
let method = req.grpc_method
if method == "AdminRpc" {
  let token = req.header("Authorization")
  if token == nil {
    return grpc_error(16, "unauthenticated")
  }
}
```

---

### req.content_length

**Signature**: `req.content_length` → `Int | Nil`

Reads the `Content-Length` header of the request and parses it as an integer. Returns `nil` if the header does not exist or the value cannot be parsed as an integer.

**Return value**: Content length in bytes, or `nil`

**Example**:
```
let cl = req.content_length
if cl != nil and cl > 1048576 {
  return deny(413, "request body too large")
}
```

---

### req.client_cert_cn

**Signature**: `req.client_cert_cn` → `Str | Nil`

Reads the Common Name (CN) field of the mTLS client certificate. Only has a value when mTLS is enabled and the client has provided a valid certificate; otherwise returns `nil`.

**Return value**: Client certificate Common Name string, or `nil`

**Example**:
```
let cn = req.client_cert_cn
if cn == nil {
  return deny(403, "client certificate required")
}
if cn != "trusted-service" {
  return deny(403, "unauthorized client: " + cn)
}
```

---

### req.client_cert_fingerprint

**Signature**: `req.client_cert_fingerprint` → `Str | Nil`

Reads the fingerprint hash (hexadecimal string) of the mTLS client certificate. Only has a value when mTLS is enabled and the client has provided a valid certificate; otherwise returns `nil`.

**Return value**: Certificate fingerprint hash string, or `nil`

**Example**:
```
let fp = req.client_cert_fingerprint
if fp != nil {
  ctx.set("client_cert_fp", fp)
}
```

---

## Request Modification (req.*)

### req.set_header

**Signature**: `req.set_header(name, value)` → `Nil`

Sets or overwrites a request header. If the header already exists, the existing value is replaced; if it does not exist, a new header is added. This modification is visible to the upstream but not to the client.

**Parameters**:
- `name` (Str) — Header name
- `value` (Str) — Header value

**Return value**: `nil`

**Example**:
```
req.set_header("X-Request-Id", uuid())
req.set_header("X-Forwarded-User", "alice")
```

---

### req.append_header

**Signature**: `req.append_header(name, value)` → `Nil`

Appends a header to the request. Does not overwrite any existing header with the same name; instead, adds a new entry with the same name.

**Parameters**:
- `name` (Str) — Header name
- `value` (Str) — Header value to append

**Return value**: `nil`

**Example**:
```
req.append_header("X-Tags", "internal")
req.append_header("X-Tags", "verified")
```

---

### req.remove_header

**Signature**: `req.remove_header(name)` → `Nil`

Removes all headers with the specified name from the request. If the header does not exist, this is a no-op.

**Parameters**:
- `name` (Str) — Name of the header to remove

**Return value**: `nil`

**Example**:
```
req.remove_header("X-Internal-Debug")
req.remove_header("Cookie")
```

---

### req.set_uri

**Signature**: `req.set_uri(uri)` → `Nil`

Rewrites the request URI forwarded to the upstream. `uri` should include the path and optionally a query string (format: `/path?key=value`). The host and scheme are not affected.

**Parameters**:
- `uri` (Str) — New URI, must start with `/`

**Return value**: `nil`

**Example**:
```
let path = req.path
let new_path = regex_replace(path, "^/v1/", "/v2/")
req.set_uri(new_path)
```

---

### req.set_host

**Signature**: `req.set_host(host)` → `Nil`

Rewrites the `Host` header of the request, affecting the Host value forwarded to the upstream.

**Parameters**:
- `host` (Str) — New Host value, e.g. `"backend.internal:8080"`

**Return value**: `nil`

**Example**:
```
req.set_host("internal-api.svc.cluster.local")
```

---

### req.set_method

**Signature**: `req.set_method(method)` → `Nil`

Changes the HTTP method of the request, preserving the supplied case. Method names are case-sensitive and are not converted to uppercase: `"post"` remains `"post"`, distinct from the standard method `"POST"`. Valid extension methods such as `"X-Custom"` also retain their case. See [HTTP method semantics](https://www.rfc-editor.org/rfc/rfc9110.html#section-9.1).

**Parameters**:
- `method` (Str) — New HTTP method, e.g. `"GET"`, `"POST"`

**Return value**: `nil`

**Errors**: The method must be a nonempty HTTP token. Empty strings or names containing spaces, control characters, separators such as `/`, or non-ASCII characters cause `ApiError`, handled by the configured `errorPolicy`. An invalid method leaves the current method unchanged.

**Example**:
```
if req.method == "HEAD" {
  req.set_method("GET")
}
```

---

## Response Reading (resp.*)

> **Note**: The following functions are only valid in the response phase (`upstreamResponsePlugins`). When called in the request phase, read functions typically return `nil`, and the status code also returns `nil`.

### resp.status

**Signature**: `resp.status` → `Int | Nil`

Reads the response status code returned by the upstream. Only has a value in the response phase; returns `nil` in the request phase (when no upstream response exists yet).

**Return value**: HTTP status code as an integer, or `nil`

**Example**:
```
let status = resp.status
if status != nil and status >= 500 {
  resp.set_header("X-Upstream-Error", "true")
}
```

---

### resp.header

**Signature**: `resp.header(name)` → `Str | Nil`

Reads the value of the specified header from the upstream response. Returns `nil` if it does not exist.

**Parameters**:
- `name` (Str) — Header name, case-insensitive

**Return value**: Header value string, or `nil`

**Example**:
```
let cache = resp.header("Cache-Control")
if cache == nil {
  resp.set_header("Cache-Control", "no-store")
}
```

---

### resp.has_header

**Signature**: `resp.has_header(name)` → `Bool`

Checks whether the upstream response contains the specified header.

**Parameters**:
- `name` (Str) — Header name

**Return value**: `true` if the header exists, `false` if it does not

**Example**:
```
if not resp.has_header("X-Request-Id") {
  let req_id = ctx.get("request_id")
  if req_id != nil {
    resp.set_header("X-Request-Id", req_id)
  }
}
```

---

### resp.header_names

**Signature**: `resp.header_names` → `List`

Returns a list of all header names in the upstream response. At most 256 names are returned; any beyond that are truncated.

**Return value**: A `List` containing all response header names

**Constraints**: At most 256 header names are returned

**Example**:
```
let names = resp.header_names
for name in names {
  if name == "X-Internal-Header" {
    resp.remove_header(name)
  }
}
```

---

### resp.content_type

**Signature**: `resp.content_type` → `Str | Nil`

Reads the `Content-Type` header value of the upstream response. Returns `nil` if it does not exist. Equivalent to `resp.header("Content-Type")`.

**Return value**: Content-Type string, or `nil`

**Example**:
```
let ct = resp.content_type
if ct != nil and regex_find(ct, "text/html") != nil {
  resp.set_header("X-Content-Type-Options", "nosniff")
}
```

---

### resp.body

`resp.body` is not supported. Configuration validation rejects scripts that
reference it because proxied upstream response bodies remain streaming-only.
Use response-header DSL operations or a per-chunk response-body plugin such as
Wasm when streaming observation is sufficient.

**Example**:
```
-- Rejected during EdgionPlugins validation:
let b = resp.body
```

---

## Response Modification (resp.*)

### resp.set_status

**Signature**: `resp.set_status(code)` → `Nil`

Modifies the final response status code sent to the client. `code` must be an integer between 200 and 599; otherwise a runtime error is raised.

**Parameters**:
- `code` (Int) — final HTTP status code, in the range 200–599

**Return value**: `nil`

**Constraints**: `code` must be an integer type and within the range 200–599; a runtime error is raised if it is out of range.

**Example**:
```
let status = resp.status
if status == 404 {
  resp.set_status(200)
  resp.set_header("X-Redirected", "true")
}
```

---

### resp.set_header

**Signature**: `resp.set_header(name, value)` → `Nil`

Sets or overwrites a response header. If the header already exists, the existing value is replaced; if it does not exist, a new header is added. This modification is visible to the client.

**Parameters**:
- `name` (Str) — Header name
- `value` (Str) — Header value

**Return value**: `nil`

**Example**:
```
resp.set_header("X-Powered-By", "Edgion")
resp.set_header("Strict-Transport-Security", "max-age=31536000; includeSubDomains")
```

---

### resp.append_header

**Signature**: `resp.append_header(name, value)` → `Nil`

Appends a header to the response without overwriting any existing header with the same name.

**Parameters**:
- `name` (Str) — Header name
- `value` (Str) — Value to append

**Return value**: `nil`

**Example**:
```
resp.append_header("Vary", "Accept-Encoding")
resp.append_header("Vary", "Accept-Language")
```

---

### resp.remove_header

**Signature**: `resp.remove_header(name)` → `Nil`

Removes all headers with the specified name from the response. If the header does not exist, this is a no-op.

**Parameters**:
- `name` (Str) — Name of the header to remove

**Return value**: `nil`

**Example**:
```
resp.remove_header("Server")
resp.remove_header("X-Powered-By")
```

---

## Context Storage (ctx.*)

Context variables are shared across plugins within the same request lifecycle. Values set in the request phase are readable in the response phase. Access-log projection requires an explicit `AccessLogExtern` declaration and follows its masking and blocklist rules; see `ctx.set` below.

### ctx.get

**Signature**: `ctx.get(key)` → `Str | Nil`

Reads the value of the specified key from the current request context. Returns `nil` if the key has not been set.

**Parameters**:
- `key` (Str) — Context variable key name

**Return value**: Value string, or `nil`

**Example**:
```
let user = ctx.get("authenticated_user")
if user != nil {
  req.set_header("X-User", user)
}
```

---

### ctx.set

**Signature**: `ctx.set(key, value)` → `Nil`

Sets a key-value pair in the current request context for subsequent plugins. `value` is stored as a string. This does not add an access-log field: declare the key with `from: ctx` in `EdgionPlugins.spec.accessLogExtern` to project it into `ext` at terminal logging. Context values are masked by default; `jwt_claims`, `oidc_claims`, and `jwe_payload` are categorically blocked. See [AccessLogExtern](../../../../../ops-guide/observability/access-log.md) for declarations, masking, and attachment rules.

**Parameters**:
- `key` (Str) — Context variable key name
- `value` (Str) — Value to store

**Return value**: `nil`

**Example**:
```
let user_id = req.path_param("userId")
ctx.set("user_id", user_id)
ctx.set("request_start", to_str(time_now()))
```

---

### ctx.remove

**Signature**: `ctx.remove(key)` → `Nil`

Removes the specified key from the context. If the key does not exist, this is a no-op.

**Parameters**:
- `key` (Str) — Key name to remove

**Return value**: `nil`

**Example**:
```
ctx.remove("temp_flag")
```

---

## String Handling

### len

**Signature**: `len(s)` → `Int`

Returns the number of Unicode characters in a string (not bytes), or the number of elements in a list. Returns `0` for `nil`.

**Parameters**:
- `s` (Str | Nil | List) — Input value

**Return value**: Character count (for strings) or element count (for lists); `0` for `nil`

**Constraints**: Only `Str`, `Nil`, and `List` are supported; other types trigger a type error.

> **Note (unit mismatch with `max_string_len`)**: `len()` reports **Unicode character count**, while the sandbox `max_string_len` limit (default 8192) and the resulting `string too long (N bytes > L bytes)` error are measured in **UTF-8 bytes**. For ASCII strings the two numbers are equal; for strings containing multi-byte characters (e.g. CJK, emoji) the byte length can be up to 4× the character length, so a `len(s) < max_string_len` guard does not guarantee that a subsequent concatenation or builtin call will pass the byte-length check.

`Body` uses the method form `req.body.len()`. The global `len(req.body)` does
not implicitly convert a Body to a string.

**Example**:
```
let token = req.header("X-Api-Key")
if token == nil or len(token) < 32 {
  return deny(401, "invalid API key length")
}
```

---

### substr

**Signature**: `substr(s, start, end)` → `Str`

Returns a substring of the string, sliced by Unicode character index (0-based, left-inclusive right-exclusive). Out-of-bounds indices are automatically clamped to the string boundaries without error. Returns an empty string for `nil` input.

**Parameters**:
- `s` (Str | Nil) — Source string
- `start` (Int) — Start character index (inclusive), 0-based
- `end` (Int) — End character index (exclusive), 0-based

**Return value**: Substring

**Example**:
```
let auth = req.header("Authorization")
if auth != nil and len(auth) > 7 {
  let scheme = substr(auth, 0, 7)
  if to_upper(scheme) == "BEARER " {
    let token = substr(auth, 7, len(auth))
    req.set_header("X-Token", token)
  }
}
```

---

### to_str

**Signature**: `to_str(v)` → `Str`

Converts scalar and container values to a string:
- `Str` → original value
- `Int` → decimal representation
- `Bool` → `"true"` or `"false"`
- `Nil` → `"nil"`
- `List` → `"[list]"`
- `Map` → a diagnostic map representation

`Body` is intentionally rejected with a type error. Use `read(offset,
max_bytes)` when an actual body string is required.

**Parameters**:
- `v` (any type) — Input value

**Return value**: String representation

**Example**:
```
let code = resp.status
if code != nil {
  resp.set_header("X-Status-Code", to_str(code))
}
```

---

### to_int

**Signature**: `to_int(s)` → `Int | Nil`

Parses a string as a 64-bit integer. If the input is already an `Int`, it is returned as-is. Returns `nil` if parsing fails or the input is of another type.

**Parameters**:
- `s` (Str | Int) — Value to parse

**Return value**: Integer value, or `nil` (if parsing fails)

**Example**:
```
let limit_str = req.header("X-Rate-Limit")
let limit = to_int(limit_str)
if limit == nil or limit <= 0 {
  limit = 100
}
```

---

### to_upper

**Signature**: `to_upper(s)` → `Str`

Returns the uppercase form of a string. Non-ASCII characters are also converted following Unicode rules.

**Parameters**:
- `s` (Str) — Input string

**Return value**: Uppercase string

**Example**:
```
let method = to_upper(req.method)
if method != "GET" and method != "POST" {
  return deny(405, "only GET/POST allowed")
}
```

---

### to_lower

**Signature**: `to_lower(s)` → `Str`

Returns the lowercase form of a string.

**Parameters**:
- `s` (Str) — Input string

**Return value**: Lowercase string

**Example**:
```
let ct = req.content_type
if ct != nil and to_lower(ct) == "application/json" {
  req.set_header("X-Is-Json", "true")
}
```

---

### trim

**Signature**: `trim(s)` → `Str`

Removes leading and trailing ASCII whitespace characters (spaces, tabs, newlines, etc.) from a string.

**Parameters**:
- `s` (Str) — Input string

**Return value**: String with leading and trailing whitespace removed

The returned string is subject to `maxStringLen`, measured in UTF-8 bytes. A
result exactly at the limit succeeds; a larger result raises the typed string
limit error and follows the configured `errorPolicy`.

**Example**:
```
let key = req.header("X-Api-Key")
if key != nil {
  key = trim(key)
  if len(key) == 0 {
    return deny(401, "empty API key")
  }
}
```

---

### split

**Signature**: `split(s, delimiter)` → `List`

Splits a string into a list using the specified delimiter. If the delimiter is an empty string, the string is split into individual characters.

**Parameters**:
- `s` (Str) — String to split
- `delimiter` (Str) — Delimiter

**Return value**: List of strings (`List`)

**Example**:
```
let roles_header = req.header("X-Roles")
if roles_header != nil {
  let roles = split(roles_header, ",")
  for role in roles {
    if trim(role) == "admin" {
      req.set_header("X-Is-Admin", "true")
    }
  }
}
```

---

### join

**Signature**: `join(list, delimiter)` → `Str`

Joins all elements of a list into a single string using the specified delimiter.

**Parameters**:
- `list` (List) — List of strings to join
- `delimiter` (Str) — Separator between elements

**Return value**: Joined string

**Constraints**: The first argument must be a `List` type; other types trigger a type error.

**Example**:
```
let parts = split(req.path, "/")
let rejoined = join(parts, "-")
req.set_header("X-Path-Slug", rejoined)
```

---

## Encoding and Decoding

### base64_encode

**Signature**: `base64_encode(s)` → `Str`

Encodes a string using standard Base64 encoding (with padding character `=`).

**Parameters**:
- `s` (Str) — String to encode

**Return value**: Base64-encoded string

**Example**:
```
let credentials = "user:password"
let encoded = base64_encode(credentials)
req.set_header("Authorization", "Basic " + encoded)
```

---

### base64_decode

**Signature**: `base64_decode(s)` → `Str | Nil`

Decodes a Base64-encoded string to the original string. Returns `nil` if the input is not valid Base64. The decoded result is interpreted as UTF-8; invalid bytes are replaced with the Unicode replacement character.

**Parameters**:
- `s` (Str) — Base64-encoded string

**Return value**: Decoded string, or `nil` (if decoding fails)

**Example**:
```
let auth = req.header("Authorization")
if auth != nil and len(auth) > 6 {
  let encoded = substr(auth, 6, len(auth))
  let decoded = base64_decode(encoded)
  if decoded != nil {
    let parts = split(decoded, ":")
    req.set_header("X-Username", parts[0])
  }
}
```

---

### url_encode

**Signature**: `url_encode(s)` → `Str`

Percent-encodes a string (URL encoding). Letters, digits, and the characters `-`, `_`, `.`, `~` are not encoded; all other characters are encoded as `%XX` (uppercase hexadecimal).

**Parameters**:
- `s` (Str) — String to encode

**Return value**: URL-encoded string

**Example**:
```
let value = req.query("redirect")
if value != nil {
  let safe = url_encode(value)
  req.set_header("X-Redirect-Encoded", safe)
}
```

---

### url_decode

**Signature**: `url_decode(s)` → `Str`

Decodes a URL-encoded string using `application/x-www-form-urlencoded` semantics: `%XX` sequences are restored to their corresponding characters, **and `+` is converted to a space**. An invalid escape leaves `%` literal; subsequent bytes are still processed, including `+` becoming space. Invalid decoded UTF-8 uses replacement characters.

> **Semantics**: this builtin matches form-body / query-string decoding, **not** RFC 3986 path decoding. In RFC 3986 a literal `+` in a path is a reserved character and must be preserved; this builtin treats it as a space. Ordinary plus handling and well-formed percent encodings of valid UTF-8 agree with query decoding, but malformed encodings can produce different results. Do not assume complete equivalence.
>
> **Security warning — do not use on URL paths for ACL / allowlist comparisons.** If you call `url_decode(req.path())` and compare the result against an allowlist that contains a literal `+`, an attacker can substitute `%20` (or vice versa) and the two distinct paths will compare equal. For path normalization in security contexts, compare against `req.path()` directly (already pingora-normalized) or use string operations (`starts_with`, `regex_match`) that do not collapse `+` and space. There is currently no RFC 3986 path-decoding builtin; the recommended path-ACL pattern is to write rules against the raw (already-normalized) path.

**Parameters**:
- `s` (Str) — URL-encoded string (intended for query strings or form bodies)

**Return value**: Decoded string

`req.query(name)` already returns a decoded query value. Do not decode it again
unless the application explicitly uses a second encoding layer.

**Example** (explicitly encoded text):
```
let encoded = "https%3A%2F%2Fexample.com%2Fcallback%3Fvalue%3Da%2Bb"
let decoded = url_decode(encoded)
req.set_header("X-Callback-Url", decoded)
```

---

### hex_encode

**Signature**: `hex_encode(s)` → `Str`

Encodes each byte of a string as two lowercase hexadecimal characters.

**Parameters**:
- `s` (Str) — String to encode

**Return value**: Hexadecimal string (lowercase)

**Example**:
```
let raw = req.header("X-Binary-Data")
if raw != nil {
  let encoded = hex_encode(raw)
  log("binary data as hex: " + encoded)
}
```

---

### hex_decode

**Signature**: `hex_decode(s)` → `Str | Nil`

Decodes a hexadecimal string to raw bytes (interpreted as UTF-8). Returns `nil` if the input contains invalid hexadecimal characters.

**Parameters**:
- `s` (Str) — Hexadecimal string (case-insensitive)

**Return value**: Decoded string, or `nil` (if input is invalid)

**Example**:
```
let hex_token = req.header("X-Hex-Token")
if hex_token != nil {
  let raw = hex_decode(hex_token)
  if raw == nil {
    return deny(400, "invalid hex token")
  }
}
```

---

## Hashing and Cryptography

### sha256

**Signature**: `sha256(s)` → `Str`

Computes the SHA-256 hash of a string, returning a 64-character lowercase hexadecimal digest.

For a request Body, use `req.body.sha256()`. The method streams the original
bytes from the already captured Body and does not require the body to fit
`maxStringLen`; it does not hash concurrently with client upload. The first
successful call for an immutable Body generation performs one bounded scan.
Later calls for that generation reuse the 32-byte digest with zero Body scan
bytes. A successful Body mutation creates a new generation and therefore
requires one new scan. The old `sha256(req.body)` form is a type error.

**Parameters**:
- `s` (Str) — Input string

**Return value**: 64-character hexadecimal string

**Example**:
```
let key = req.header("X-Api-Key")
if key != nil {
  let hashed = sha256(key)
  req.set_header("X-Api-Key-Hash", hashed)
  req.remove_header("X-Api-Key")
}
```

---

### md5

**Signature**: `md5(s)` → `Str`

Computes the MD5 hash of a string, returning a 32-character lowercase
hexadecimal digest. MD5 is collision-broken and provides no adversarial
integrity guarantee. Use it only for legacy protocol compatibility, cache keys,
or accidental-corruption checks; never use it for signatures, authentication,
password storage, or attacker-controlled integrity decisions.

**Parameters**:
- `s` (Str) — Input string

**Return value**: 32-character hexadecimal string

**Example**:
```
let body_hash = md5(req.path + to_str(time_now()))
req.set_header("X-Cache-Key", body_hash)
```

---

### hmac_sha256

**Signature**: `hmac_sha256(key, data)` → `Str`

Generates a message authentication code using HMAC-SHA256 and returns a
64-character lowercase hexadecimal string. For raw request Body bytes, use
`req.body.hmac_sha256(key)`.

**Parameters**:
- `key` (Str) — Secret key string
- `data` (Str) — Data to sign

**Return value**: 64-character hexadecimal HMAC digest

This function generates a MAC. Ordinary DSL `==` is not constant-time, so do
not use either HMAC function to verify an attacker-supplied tag. Use a
dedicated authentication plugin or external verifier for inbound signature
verification.

**Example — generate an upstream signature**:
```
let secret = "my-secret-key"
let timestamp = req.header("X-Timestamp")
if timestamp != nil {
  let signature = hmac_sha256(secret, timestamp + ":" + req.path)
  req.set_header("X-Upstream-Signature", signature)
}
```

---

## Regular Expressions

The `.matches("pattern")` method works on both `Str` and `Body`. It uses a
bounded incremental DFA and a compile-time literal pattern, so matches may span
storage chunks without materializing a Body. The streaming dialect rejects
constructs the DFA cannot execute, including Unicode word boundaries; use an
explicit ASCII boundary such as `(?-u:\b)`. `regex_find` and `regex_replace`
remain string-only because they return or construct strings.

### regex_find

**Signature**: `regex_find(s, pattern)` → `Str | Nil`

Finds the first match of a regular expression in a string and returns the matched substring. Returns `nil` if there is no match. Regular expression syntax follows the Rust `regex` crate (RE2 style, no backtracking support).

**Parameters**:
- `s` (Str) — String to search
- `pattern` (Str) — Regular expression pattern

**Return value**: First matched substring, or `nil`

**Constraints**: The pattern must be a string literal. Both the 1024-byte length limit and regex syntax are validated at compile time.

**Example**:
```
let ct = req.content_type
if ct != nil {
  let mime = regex_find(ct, "^[^;]+")
  if mime != nil {
    req.set_header("X-Mime-Type", trim(mime))
  }
}
```

---

### regex_replace

**Signature**: `regex_replace(s, pattern, replacement)` → `Str`

Replaces all occurrences of the regular expression match in a string with the specified replacement string. Capture group references such as `$1` and `$2` are supported.

**Parameters**:
- `s` (Str) — Original string
- `pattern` (Str) — Regular expression pattern
- `replacement` (Str) — Replacement string; capture group references such as `$1` may be used

**Return value**: Replaced string

**Constraints**: The pattern must be a string literal. Both the 1024-byte length limit and regex syntax are validated at compile time.

**Example**:
```
let path = req.path
let new_path = regex_replace(path, "^/api/v1/", "/api/v2/")
req.set_uri(new_path)
```

---

## Time

### time_now

**Signature**: `time_now()` → `Int`

Returns the current Unix timestamp (seconds elapsed since 1970-01-01 00:00:00 UTC).

**Return value**: Unix timestamp (integer, in seconds)

**Example**:
```
let ts = time_now()
let request_ts = to_int(req.header("X-Timestamp"))
if request_ts == nil or ts - request_ts > 300 {
  return deny(401, "request expired")
}
```

---

### time_format

**Signature**: `time_format(timestamp, format)` → `Str`

Formats a Unix timestamp as a string. The format string uses the `chrono` crate format specification (similar to C's `strftime`). Returns `nil` if the timestamp is invalid.

**Parameters**:
- `timestamp` (Int) — Unix timestamp (seconds)
- `format` (Str) — chrono format string, e.g. `"%Y-%m-%dT%H:%M:%SZ"`

**Return value**: Formatted time string

**Constraints**: The format string must be at most 256 bytes; a runtime error is triggered if it exceeds this limit. An invalid format specifier (e.g. an unknown `%X`) also raises a runtime error rather than returning a string.

**Common format characters**:
- `%Y` — 4-digit year
- `%m` — 2-digit month (01–12)
- `%d` — 2-digit day (01–31)
- `%H` — 2-digit hour (00–23)
- `%M` — 2-digit minute (00–59)
- `%S` — 2-digit second (00–59)

**Example**:
```
let ts = time_now()
let date_str = time_format(ts, "%Y-%m-%dT%H:%M:%SZ")
req.set_header("X-Request-Time", date_str)
```

---

## JSON

### json_get

**Signatures**:

- `json_get(json_str, "$.path")` → `Str | Int | Bool | Nil`
- `req.body.json_get("$.path")` → `Str | Int | Bool | Nil`

Selects a value from a string or request Body using a compile-time definite
JSONPath. The accepted subset is root `$`, dot members (`$.user.name`), quoted
members (`$["name.with.dots"]`), and non-negative array indices
(`$.items[0].name`). Wildcards, filters, recursive descent, unions, slices,
negative indices, and dynamic paths are rejected at compile time.

**Parameters**:
- `json_str` (Str) — JSON string
- `path` (Str literal) — Definite JSONPath

**Return value**: Extracted value (`Str`, `Int`, `Bool`, or `Nil`)

**Constraints**:
- Paths are at most 1024 UTF-8 bytes and 32 segments
- The complete JSON document is validated, but Body input retains only the selected value rather than building a complete DOM
- JSON numbers that can be represented as `i64` are returned as `Int`; otherwise they are returned as `Str`; array and object nodes are returned as their JSON string representation
- The selected string or serialized array/object must fit `maxStringLen`
- A missing path, structural mismatch, or invalid JSON returns `nil`; request-body storage failure is fixed fail-close

**Example**:
```
let payload = req.header("X-Jwt-Claims")
if payload != nil {
  let user_id = json_get(payload, "$.sub")
  let role = json_get(payload, "$.role")
  if user_id != nil {
    req.set_header("X-User-Id", to_str(user_id))
  }
  if role == "admin" {
    req.set_header("X-Is-Admin", "true")
  }
}
```

---

### req.body.json_select

**Signature**:
`req.body.json_select({"outputName": "$.path", ...})` → `Map`

Selects several definite JSONPaths from the current request Body with one
complete Body scan and one JSON parse. The argument must be a non-empty map
literal: output names and paths cannot be computed at runtime.

The result contains every declared output name in declaration order. Missing
paths, traversal mismatches, and JSON `null` become `Nil`. Duplicate paths are
allowed under different output names and are captured once. Duplicate JSON
object keys use the last occurrence, matching `json_get`.

**Constraints**:

- 1–64 entries, further limited by `maxMapSize`
- unique output names up to 256 UTF-8 bytes
- paths up to 1,024 UTF-8 bytes and 32 segments
- each selected string or serialized subtree, and the live aggregate selected
  string payload, is bounded by `maxStringLen`
- the complete JSON document is validated; invalid JSON returns a Map whose
  values are all `Nil`
- Body storage and scan-budget failures remain fixed fail-close

Numbers use the same conversion as `json_get`: values fitting `i64` become
`Int`; other numbers use its canonical `Str` conversion. Exact source-token
spelling is not promised.

Prefer `json_select` whenever a script needs several Body fields. Repeated
`json_get` calls each parse and scan the complete Body and each consume a Body
scan pass.

```dsl
let values = req.body.json_select({
  "id": "$.user.id",
  "name": "$.user.name",
  "role": "$.user.role"
})

if values["id"] != nil {
  req.set_header("X-User-Id", to_str(values["id"]))
}
```

---

## Network

### http.call

**Signature**: `http.call(method, url, body [, timeout_ms [, headers]])` → `Map`

Sends an outbound HTTP request from the gateway to an arbitrary HTTP/HTTPS endpoint. The call is synchronous from the script's perspective and returns when a response is received or the timeout expires.

**Parameters**:
- `method` (Str) — HTTP method, e.g. `"GET"`, `"POST"`, `"PUT"`
- `url` (Str) — Full URL including scheme and host, e.g. `"https://api.example.com/v1/check"`
- `body` (Str | Body | Nil) — A string request body, the opaque `req.body`, or `nil` for no body
- `timeout_ms` (Int, optional) — Per-request timeout in milliseconds; capped to `httpMaxTimeoutMs` (default 5000)
- `headers` (Map, optional) — Additional request headers as a `Map` of string key-value pairs

**Return value**: A `Map` with the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `status` | Int | HTTP response status code; `0` for network, timeout, dial-time egress rejection, response-stream failure, or bounded-response validation failure |
| `body` | Str | Response body as UTF-8 text; empty string if the call failed or the body is empty |
| `headers` | Map | Retained response headers; keys are lowercase, and a later duplicate replaces an earlier value only while raw-header traversal remains open |

Network failures, timeouts, response-stream failures, bounded-response overflows, and
DNS-resolved addresses rejected at dial time return `status = 0` with an empty body
and empty headers. Invalid arguments,
malformed or unsupported URLs, direct IP literals rejected by the SSRF precheck, and
caller-supplied framing headers on an opaque `Body` call raise normal DSL runtime
errors. Opaque-Body access, read, scan-budget, and limit failures raise the fixed
fail-close `BodyAccess` error.

**Constraints**:
- Only `http` and `https` URL schemes are accepted; other schemes (e.g. `file://`, `ftp://`) raise an error.
- When `httpBlockPrivateIps: true`, loopback, private, link-local, and unspecified destinations are blocked. Direct IP literals are rejected before sending; hostnames are checked after DNS resolution against the exact addresses used for dialing, including on redirect hops. Network-level egress controls remain recommended as defense in depth.
- Only the third argument accepts the opaque `Body` value, normally written as `req.body`. The runtime takes an immutable call-time snapshot and pull-streams its raw bytes from memory, spill files, and mutation segments without materializing the complete body.
- Opaque `Body` size is capped by `min(httpMaxRequestBodyBytes, gateway physical request-body cap)`. The configured field defaults to 1 MiB and may be explicitly raised to at most 32 MiB. Bytes fetched from the snapshot count against the cumulative `maxBodyScanPasses` budget.
- On an opaque `Body` call, caller headers must not contain `Content-Length` or `Transfer-Encoding`; either header is rejected so the runtime exclusively controls request framing. An empty opaque `Body` sends `Content-Length: 0`; `nil` attaches no body.
- The response body hard cap is `min(httpMaxResponseBodyBytes, max_string_len)` (defaults: 1 MiB and 8 KiB respectively). A declared or streamed overflow fails the complete call with the status-zero result; no truncated prefix is returned. A body exactly at the cap succeeds only after end-of-stream, and lossy UTF-8 expansion must also fit `max_string_len`.
- Response headers are traversed in raw iteration order. Before inspecting each raw header, traversal stops if the retained `headers` Map already has `max_map_size` names. Within the inspected raw prefix, non-UTF-8 values are skipped and a duplicate replaces an earlier value only if reached before the Map becomes full. The ignored tail is not examined. Names are lowercased, and every retained name and value must fit `max_string_len`; otherwise the complete call returns the status-zero failure result.
- Retained response-header names plus values have a fixed cumulative 1 MiB byte cap, checked before copying each entry. A replacement refunds the previous value's bytes and counts the name only once. Overflow fails the complete call with the same status-zero result, before reading its body. This is separate from the canonical client's decoded HTTP/2 response-header-list limit (16 KiB by default, at most 64 KiB).
- The actual timeout applied is `min(timeout_ms, httpMaxTimeoutMs)`.
- `Str` and `Nil` calls follow at most 10 redirects. When `httpBlockPrivateIps: true`, both the direct-literal and dial-time resolved-address checks apply to every redirect hop. Opaque `Body` calls never follow redirects and return the 3xx response directly.

**Configuration fields** (inside `config`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `httpMaxTimeoutMs` | Integer | `5000` | Hard cap on per-call timeout (`1`–`60000` ms) |
| `httpMaxRequestBodyBytes` | Integer | `1048576` | Maximum opaque `Body` request size; range `1`–`33554432`, further capped by the gateway physical request-body limit |
| `httpMaxResponseBodyBytes` | Integer | `1048576` | Maximum response body size; range `1`–`1048576` bytes, and the complete call fails if exceeded |
| `httpBlockPrivateIps` | Boolean | `false` | Block outbound calls to private/loopback/link-local/unspecified IP addresses, including hostname results checked at dial time |

**Example — call a health endpoint and branch on status**:
```
let r = http.call("GET", "http://internal-service/health", nil, 1000)
if r["status"] == 200 {
  req.set_header("X-Backend-Healthy", "true")
} else {
  return deny(503, "backend unavailable")
}
```

**Example — POST JSON with custom headers**:
```
let hdrs = {"Content-Type": "application/json", "X-Api-Key": req.header("X-Api-Key")}
let r = http.call("POST", "https://auth.example.com/verify", "{\"token\":\"abc\"}", 2000, hdrs)
if r["status"] != 200 {
  return deny(401, "auth check failed")
}
```

**Example — stream the current request body without full materialization**:
```
let hdrs = {"Content-Type": req.content_type}
let r = http.call("POST", "https://scanner.example.com/check", req.body, 2000, hdrs)
if r["status"] != 200 {
  return deny(403, "request rejected")
}
```

**Example — read a response header**:
```
let r = http.call("GET", "https://api.example.com/quota", nil, 2000)
let remaining = r["headers"]["x-ratelimit-remaining"]
if remaining != nil {
  resp.set_header("X-Quota-Remaining", remaining)
}
```

---

## Redis (redis.*)

All `redis.*` functions are backed by a pre-configured LinkSys Redis connection pool. The connection pool is resolved at request time via the `linkSysRefs` config field. Each `redis.*` call takes a `link` (the alias name) as its first argument to identify which connection to use.

**Error contract**: Every `redis.*` function returns a `Map { ok: Bool, value: <type>, error: Str | Nil }`. The function **never raises a runtime error** and **never panics**. Script authors decide how to handle failures (fail-open, fail-close, or graceful degradation).

- `ok: true` — The operation succeeded; `value` holds the result; `error` is `nil`.
- `ok: false` — A Redis connection, operation, argument, or result error occurred; `value` is `nil`; `error` holds a fixed public class. It never contains the alias, key, field, value, dependency address, or dependency error text.
- Common classes are `invalid_argument`, `unknown_link`, `provider_not_ready`, `dependency_error`, and `result_too_large`; `redis.rate_limit` also uses `unknown_policy` and `timeout`.
- Every class is subject to `maxStringLen`. If the configured limit is too small for the specific class, `error` is the generic one-byte class `e`; sensitive text is never partially truncated into the result.

**Configuration field** (inside `config`):

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `linkSysRefs` | Vec<{namespace, name}> | Required when `redis.*` is used | List of LinkSys Redis resource references. Each element has `namespace` and `name` fields; the `name` is the alias used in `redis.*` calls. Configuring this field is mandatory if any `redis.*` builtin appears in the script; a missing `linkSysRefs` causes the plugin to fail at load time. |

Indexed Boolean results use explicit comparisons: `r["ok"] == false` or
`r["value"]["allowed"] == false`. In the current grammar, unary `!` binds
before indexing; do not write `!r["ok"]`.

### redis.get

**Signature**: `redis.get(link, key)` → `Map { ok: Bool, value: Str | Nil, error: Str | Nil }`

Reads the value of a string key. Returns `value: nil` when the key does not exist.

The returned `value` is silently truncated at `max_string_len` (default 8 KiB, the same cap that applies to every other DSL string value); the call still succeeds and does not raise. Operators who need to read larger values must raise `max_string_len`.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name

**Example — fail-close**:
```
let r = redis.get("cache", "feature:dark-mode")
if r["ok"] == false {
  log_warn("redis error: " + r["error"])
  return deny(503, "service unavailable")
}
if r["value"] == "on" {
  req.set_header("X-Feature-Dark-Mode", "true")
}
```

---

### redis.set

**Signature**: `redis.set(link, key, value)` or `redis.set(link, key, value, ttl_ms)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

Sets a string key. Optionally sets a TTL in milliseconds.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name
- `value` (Str) — Value to store
- `ttl_ms` (Int, optional) — Time-to-live in milliseconds; `nil` or `0` means no expiry

**Example**:
```
let r = redis.set("cache", "session:" + req.header("X-Session-Id"), "active", 300000)
if r["ok"] == false {
  log_warn("redis set failed: " + r["error"])
}
```

---

### redis.del

**Signature**: `redis.del(link, key)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

Deletes a key. Returns `value: true` if the key was deleted, `value: false` if the key did not exist.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name

---

### redis.exists

**Signature**: `redis.exists(link, key)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

Checks whether a key exists. Returns `value: true` if the key exists, `value: false` otherwise.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name

---

### redis.ttl

**Signature**: `redis.ttl(link, key)` → `Map { ok: Bool, value: Int, error: Str | Nil }`

Returns the remaining time-to-live of a key in seconds.

| `value` | Meaning |
|---------|---------|
| `>= 0` | Remaining TTL in seconds |
| `-1` | Key exists but has no expiry |
| `-2` | Key does not exist |

`ok: false` is returned only on a connection error.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name

---

### redis.expire

**Signature**: `redis.expire(link, key, seconds)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

Sets a TTL on an existing key. Returns `value: true` if the timeout was set, `value: false` if the key does not exist.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name
- `seconds` (Int) — TTL in seconds

---

### redis.incr

**Signature**: `redis.incr(link, key)` → `Map { ok: Bool, value: Int, error: Str | Nil }`

Atomically increments the integer value of a key by 1. Creates the key with value `1` if it does not exist.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name

**Example — cumulative counter** (no time window or automatic reset; use
`redis.rate_limit` below for rate limiting):
```
let r = redis.incr("cache", "rate:" + req.remote_ip)
if r["ok"] == false {
  log_warn("redis unavailable")
  return deny(503, "service unavailable")
}
if r["value"] > 100 {
  return deny(429, "cumulative quota exceeded")
}
```

---

### redis.rate_limit

**Signature**: `redis.rate_limit(link, policy, key, cost)` → `Map { ok, value, error }`

Atomically evaluates one named Redis rate-limit policy. Configure a Redis LinkSys
resource named `default/cache`, then attach this EdgionPlugins resource to your
HTTPRoute using ExtensionRef:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-rate-limit
  namespace: default
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: limited-api
        errorPolicy: deny
        linkSysRefs:
          - name: cache
            namespace: default
        rateLimitPolicies:
          - name: requests
            link: cache
            namespace: public-api
            algorithm: SlidingWindow
            rate: 100
            interval: 1s
            timeoutMs: 500
        source: |
          let r = redis.rate_limit("cache", "requests", req.remote_ip, 1)
          if r["ok"] == false {
            log_warn("rate limit unavailable")
            return deny(503, "service unavailable")
          }
          if r["value"]["allowed"] == false {
            return deny(429, "rate limit exceeded")
          }
          return next()
```

| Argument | Type | Meaning |
|---|---|---|
| `link` | Str | Declared `linkSysRefs[].name`; must match the policy's `link` |
| `policy` | Str | Name from `rateLimitPolicies` |
| `key` | Str | Business bucket key, at most 4096 UTF-8 bytes; empty selects a global bucket |
| `cost` | Int | Positive weighted consumption, 1–1000000; strings are not coerced |

At most 64 policies may be configured, with unique names. Names and namespaces
contain 1–128 ASCII letters, digits, or `-_.:/`. The explicit `namespace` controls
sharing: equal namespace, policy name, algorithm, rate, interval and key on the
same Redis database share counters across plugins. Use different namespaces for
independent rules. Changing algorithm, rate or interval starts a new counter
generation; changing only timeout preserves counters. Link aliases are local names,
not a guarantee of separate Redis storage. The DSL prefix is separate from the
HTTP/Stream rate-limit plugins. Do not write other values into DSL-owned keys.

`algorithm` accepts `SlidingWindow` (default), `FixedWindow`, or `TokenBucket`.
`rate` is 1–1000000 units per `interval`; TokenBucket capacity equals rate.
Intervals use duration strings, from `100ms` through `2147483647ms`, with whole
milliseconds. `timeoutMs` defaults to 500 and accepts 1–30000 milliseconds.

Successful execution has `ok=true`, `error=nil`, and these `value` fields:

| Field | Meaning |
|---|---|
| `allowed` | Boolean admission decision; denial is not a dependency error |
| `current` | Current count for window algorithms; remaining token balance for TokenBucket |
| `remaining` | Remaining units after allowance; zero on denial |
| `limit` | Configured rate |
| `retry_ms` | Existing algorithm's retry estimate in milliseconds, zero on allowance |

The algorithms preserve their existing behavior: SlidingWindow uses a weighted
counter approximation, FixedWindow permits boundary bursts, and TokenBucket
refills in whole tokens. Retry delay is not a reservation or exact next-admission
time. A cost larger than rate can never be allowed by that policy.

Failures return `ok=false`, `value=nil`, and a fixed `error`:
`invalid_argument`, `unknown_policy` (also a policy/link mismatch),
`unknown_link`, `provider_not_ready`, `dependency_error`, or `timeout`.
Bad types and out-of-range cost/key values are rejected before Redis I/O.
Policy configuration is validated at load time. Unknown/dynamic aliases still
require runtime handling; static alias checking is best-effort.

Each call consumes the normal VM call budget and requires `maxMapSize >= 5`
for its successful result. Fixed failure classes obey `maxStringLen`; a class that
does not fit becomes the generic class `e` while preserving the failure Map. Other
VM limit failures follow `errorPolicy`; the example sets it to `deny` and separately
handles result-map failures.

The timeout bounds the complete operation, including a script-cache miss.
Timeout/cancellation can happen after Redis has charged quota: do not retry on
the assumption that nothing executed. No application retry is added after uncertain
execution; the existing LinkSys transport behavior still applies. Multiple calls
are separate atomic operations, not one transaction. Client replacement is
observed on each call. Gateway wall-clock skew can affect windows/refill timing;
numeric bounds do not synchronize clocks or validate externally corrupted state.

---

### redis.incr_by

**Signature**: `redis.incr_by(link, key, delta)` → `Map { ok: Bool, value: Int, error: Str | Nil }`

Atomically increments the integer value of a key by `delta`. `delta` may be negative (acts as DECRBY).

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name
- `delta` (Int) — Amount to add (may be negative)

---

### redis.hget

**Signature**: `redis.hget(link, key, field)` → `Map { ok: Bool, value: Str | Nil, error: Str | Nil }`

Gets the value of a hash field. Returns `value: nil` when the key or field does not exist.

The returned `value` is silently truncated at `max_string_len` (default 8 KiB), the same as `redis.get`.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Hash key name
- `field` (Str) — Field name

---

### redis.hset

**Signature**: `redis.hset(link, key, field, value)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

Sets a field in a hash. Returns `value: true` on success.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Hash key name
- `field` (Str) — Field name
- `value` (Str) — Field value

---

### redis.hdel

**Signature**: `redis.hdel(link, key, field)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

Deletes a field from a hash. Returns `value: true` if the field existed and was deleted, `value: false` otherwise.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Hash key name
- `field` (Str) — Field name

---

### redis.hgetall

**Signature**: `redis.hgetall(link, key)` → `Map { ok: Bool, value: Map, error: Str | Nil }`

Gets all fields and values of a hash. Returns an empty `Map` when the key does not exist. `ok: false` is returned on a connection error, or when the hash contains more entries than the `max_map_size` limit (in which case the oversize `Map` is not constructed and `error` is `result_too_large`, subject to the generic-class rule above). Each entry's key and value string is independently truncated at `max_string_len` (default 8 KiB), the same as `redis.get`; the call still succeeds.

The original field and value bytes together must also fit a fixed 1 MiB cap. This check uses the untruncated reply and runs before converting redis-rs's result to an owned string table. Exceeding either the entry count or byte cap returns `ok: false`, `value: nil`, and `error: "result_too_large"` (or `"e"` for a small string budget). Admitted results retain the per-string truncation described above. Raising `maxStringLen` does not raise the cumulative cap.

**Dependency boundary**: redis-rs 1.8.1 receives and aggregates the complete HGETALL reply before Edgion can inspect it. The cap checks the raw reply before the Edgion string table and DSL Map construction for oversized results; it does not bound Redis wire buffering or the decoded frame (including a native RESP3 Map). Keep server-side hashes bounded; this check alone is not a total-memory guarantee.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Hash key name

**Example**:
```
let r = redis.hgetall("cache", "user:config:1234")
if r["ok"] {
  let theme = r["value"]["theme"]
  if theme != nil {
    resp.set_header("X-User-Theme", theme)
  }
}
```

---

### redis.hexists

**Signature**: `redis.hexists(link, key, field)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

Checks whether a field exists in a hash. Returns `value: true` if the field exists, `value: false` otherwise.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Hash key name
- `field` (Str) — Field name

---

### ip_in_cidr

**Signature**: `ip_in_cidr(ip, cidr)` → `Bool`

Checks whether the specified IP address belongs to the given CIDR range. Both IPv4 and IPv6 are supported. Comparing an IPv4 address against an IPv6 CIDR (or vice versa) always returns `false`. If the IP address or CIDR format is invalid, returns `false` (no error is raised).

**Parameters**:
- `ip` (Str) — IP address string, e.g. `"192.168.1.100"` or `"::1"`
- `cidr` (Str) — CIDR range string, e.g. `"192.168.0.0/16"` or `"fc00::/7"`

**Return value**: `true` if the IP is within the range, `false` if it is not or the format is invalid

**Example**:
```
let ip = req.direct_peer_ip
if ip_in_cidr(ip, "10.0.0.0/8") or ip_in_cidr(ip, "172.16.0.0/12") or ip_in_cidr(ip, "192.168.0.0/16") {
  req.set_header("X-Private-Network", "true")
} else {
  return deny(403, "access from public networks is not allowed")
}
```

---

## Control Flow Functions

The following functions terminate the current request processing pipeline. They are special return statements in the script and are typically used with the `return` keyword.

### deny

**Signature**: `deny(status, body)` — terminates the request

Returns the specified HTTP error response to the client and terminates processing. If `status` is out of range, it is automatically downgraded to 500. If `body` is `nil`, the response body is empty.

**Parameters**:
- `status` (Int) — final HTTP status code, in the range 200–599
- `body` (Str | Nil) — Response body string; pass `nil` for no response body

**Example**:
```
let token = req.header("X-Api-Key")
if token == nil {
  return deny(401, "API key required")
}
if len(token) < 16 {
  return deny(400, "API key too short")
}
```

---

### grpc_error

**Signature**: `grpc_error(code, message)` — terminates processing with a gRPC response (request or response phase)

Returns a standard gRPC error to the client, sending `grpc-status` and `grpc-message` via HTTP/2 trailers. `code` is a gRPC status code in the range 0–16; values outside this range are automatically downgraded to 13 (INTERNAL).

**Parameters**:
- `code` (Int) — gRPC status code (0–16)
- `message` (Str | Nil) — Error message; pass `nil` for no message

In `requestPlugins`, `grpc_error()` sends a direct trailers-only gRPC response through the canonical terminal writer. In `upstreamResponsePlugins`, it aborts the upstream response and carries the exact gRPC code and optional message to that same writer; the upstream response is not forwarded. In both phases, the wire HTTP status is `200`, while the gRPC application status is recorded separately in the access log as `grpc_status`. The code is not reconstructed from an HTTP status.

**Common gRPC status codes**:
- `0` — OK
- `1` — CANCELLED
- `2` — UNKNOWN
- `3` — INVALID_ARGUMENT
- `5` — NOT_FOUND
- `7` — PERMISSION_DENIED
- `13` — INTERNAL
- `16` — UNAUTHENTICATED

**Example**:
```
if req.protocol == "grpc" {
  let token = req.header("Authorization")
  if token == nil {
    return grpc_error(16, "unauthenticated: missing token")
  }
}
```

---

### terminate

**Signature**: `terminate()` — terminates the request

Immediately stops the script and subsequent plugins with an explicit HTTP `502` and an empty response body. gRPC requests receive HTTP `200` with `grpc-status: 13` (INTERNAL). It does not silently close the connection and is independent of `errorPolicy`, including `ignore`. Use `deny(status, body)` for a different HTTP status or body, or `grpc_error()` for a specific gRPC code and message.

In the response stage, the gateway replaces an uncommitted ordinary HTTP upstream response, discarding its headers and body, including upstream-header edits made by `resp.set_header()` in that stage. Response-header operations queued during the request stage are applied to the local response. gRPC, switched-protocol, and established-tunnel responses stop through the existing denial path. HTTP/1 response-stage local replacement closes the connection; connection reuse is not guaranteed.

**Behavior change**: The HTTP status remains `502`, but the former fallback HTML body is now empty. The DSL decision log is `Deny 502`, and a valid call no longer produces `PluginTerminateWithoutResponse`; the response dispatcher may still append replacement or denial records.

**Example**:
```
return terminate()
```

---

## Other Utilities

### uuid

**Signature**: `uuid()` → `Str`

Generates a random UUID v4 in the standard 8-4-4-4-12 hexadecimal format, e.g. `"550e8400-e29b-41d4-a716-446655440000"`.

**Return value**: UUID v4 string (36 characters, including hyphens)

**Example**:
```
if not req.has_header("X-Request-Id") {
  req.set_header("X-Request-Id", uuid())
}
```

---

### range

**Signature**: `range(start, end)` → `List`

Generates an integer sequence from `start` (inclusive) to `end` (exclusive), with elements as numeric strings.

**Parameters**:
- `start` (Int) — Start value (inclusive)
- `end` (Int) — End value (exclusive)

**Return value**: List of numeric strings

**Constraints**: At most `min(max_list_len, 1000)` elements are generated (where `max_list_len` is the per-plugin VM limit, default 1024); any beyond that are silently truncated.

**Example**:
```
let nums = range(1, 6)
let joined = join(nums, ",")
req.set_header("X-Page-Range", joined)
```

---

### log

**Signature**: `log(msg)` → `Nil`

Records a log message at the INFO level. The log content is appended to the DSL execution log for the current request and can be viewed via the `dsl_log` field in the access log.

**Parameters**:
- `msg` (Str) — Log message

**Return value**: `nil`

**Example**:
```
let user = req.header("X-User-Id")
log("processing request for user: " + to_str(user))
```

---

### log_fmt

**Signature**: `log_fmt(template_literal, value1, ...)` → `Nil`

Appends a formatted entry to the current request's plugin access log. The template
must be a string literal, with one `{}` placeholder per value. Values pass through
the configured sensitive-data masking; literal template text is not masked.

Every builtin call is limited to 255 total arguments and must also satisfy its
function-specific argument count. For `log_fmt`, this means one template plus at
most 254 values. Excess arguments are rejected during compilation, including
configuration reload. `maxStackDepth` is a separate execution limit: raising it
does not increase the argument capacity. Precompiled bytecode is also checked for
valid argument counts, consistent operand-stack heights, and a template loaded
directly from a string constant before execution. Dynamic templates are rejected
on both configuration paths.

```
log_fmt("user {}", req.header("X-User-Id"))
```

---

### log_warn

**Signature**: `log_warn(msg)` → `Nil`

Records a log message at the WARN level. The message is written to the Gateway's structured logging system and appended to the DSL execution log (with the prefix `warn:`).

**Parameters**:
- `msg` (Str) — Warning message

**Return value**: `nil`

**Example**:
```
let cl = req.content_length
if cl != nil and cl > 524288 {
  log_warn("large request body: " + to_str(cl) + " bytes")
}
```

---

### log_error

**Signature**: `log_error(msg)` → `Nil`

Records a log message at the ERROR level. The message is written to the Gateway's structured logging system and appended to the DSL execution log (with the prefix `error:`).

**Parameters**:
- `msg` (Str) — Error message

**Return value**: `nil`

**Example**:
```
let sig = req.header("X-Signature")
if sig == nil {
  log_error("missing signature header, rejecting request from: " + req.remote_ip)
  return deny(401, "signature required")
}
```

---

## Bounded XML primitives

`Xml` is an immutable element, attribute or text handle. Element and attribute
names use a namespace URI and local name; prefixes are presentation only.
Children retain mixed text/element order and repeated elements. Ordinary
attributes retain source order; namespace declarations are scope metadata and
are excluded from attribute iteration. Map keys in constructors are labels;
Map insertion order supplies the attribute/child sequence.

| Function | Arguments | Result |
|----------|-----------|--------|
| `xml.parse(source)` | `Str` or `req.body` | Root `Xml` element; malformed/unsupported input returns `nil` |
| `xml.kind(node)` | `Xml\|Nil` | `"element"`, `"attribute"`, `"text"`, or `nil` |
| `xml.name(node)` | `Xml\|Nil` | Local name; text/missing returns `nil` |
| `xml.namespace(node)` | `Xml\|Nil` | Namespace URI; unqualified name is `""`; text/missing returns `nil` |
| `xml.value(node)` | `Xml\|Nil` | Attribute/text value or concatenated descendant element text; missing returns `nil` |
| `xml.child_count(element)` | Element `Xml\|Nil` | Ordered child count, including text; missing returns `nil` |
| `xml.child(element, index)` | Element, zero-based `Int` | Ordered child; negative/missing index returns `nil` |
| `xml.find(element, namespace, name, occurrence)` | Element, two `Str`, zero-based `Int` | Matching direct element child; missing/negative occurrence returns `nil` |
| `xml.attribute_count(element)` | Element `Xml\|Nil` | Ordinary attribute count; missing returns `nil` |
| `xml.attribute(element, index)` | Element, zero-based `Int` | Attribute `Xml`; missing/negative index returns `nil` |
| `xml.attr_value(element, namespace, name)` | Element, two `Str` | Attribute `Str` or `nil` |
| `xml.text(value)` | `Str` | Text `Xml` |
| `xml.attr(namespace, name, value)` | Three `Str` | Attribute `Xml` |
| `xml.element(namespace, name, attributes, children)` | Two `Str`, two ordered `Map` | Element `Xml`; attributes must contain attribute nodes, children contain element/text nodes |
| `xml.stringify(element)` | Element `Xml\|Nil` | Complete serialized `Str`; missing returns `nil`; also obeys `maxStringLen` |
| `xml.set_body(element)` | Element `Xml` | `nil`; atomically replaces the request body after complete bounded serialization |

XML 1.0 with UTF-8 input is supported. CDATA becomes text; comments, processing
instructions and declarations are validated and omitted from the value model.
In the supported declaration subset, `<?xml` must be followed by an ASCII space;
a tab, CR or LF at that position is unsupported and parse returns `nil`.
XML newline and attribute whitespace normalization apply; numeric references
retain their referenced whitespace. Serialization uses deterministic explicit
prefixes, escapes markup and normalization-sensitive characters, and preserves
expanded names and semantic text rather than source formatting.
Equal namespace URIs share a prefix on each element. Generated namespace
declarations also obey the 64-per-element ceiling; serializing a subtree alone
can require extra local bindings and fail with the namespace limit.

DTD (including empty DTD), custom/external entities, XInclude, invalid XML
characters and unsupported encoding/version declarations return `nil` from
parse. Only predefined and valid numeric character references are decoded.
No entity, stylesheet, schema or include URL is fetched. XML Schema/WSDL
validation and automatic XML-to-JSON mappings are outside this API.

Local names must be NCNames. Constructors reject invalid characters/names,
duplicate expanded attribute names, invalid node kinds and reserved `xmlns` or
XInclude namespaces with `ApiError`. The standard XML namespace uses `xml`.
Non-XML/non-nil getter inputs and incompatible argument types raise `TypeError`.
`Xml` never implicitly converts to a string, including `to_str`, concatenation,
headers, logging and `ctx.set`; use `xml.value` or `xml.stringify`. Equality
means the same immutable handle, not structural equality.

Hard ceilings: 1 MiB source, serialized output and aggregate decoded subtree
payload; depth 32; 4,096 expanded element/text nodes and 4,096 attributes;
64 ordinary attributes and 64 namespace declarations per element; 256-byte
prefix/local names and 1,024-byte namespace URIs. Repeated shared handles count
each logical occurrence. Parsing, construction, traversal and serialization
consume the existing cumulative VM value-work budget; limits never truncate.

`xml.parse(req.body)` reads the current shared request-body generation, including
disk-backed bodies, under the resolved physical cap and `maxBodyScanPasses`.
Malformed input is still `nil`, so authorization scripts must reject it
explicitly. Body storage/scan/XML/work-limit failures and `xml.set_body`
serialization/publication failures return the fixed 500 `Request body processing
failed` regardless of `errorPolicy`. String XML limits follow the normal DSL
policy; string result overflow is `StringTooLong`. `xml.set_body` is request-stage
only, triggers body capture even for string-only construction, and leaves the
previous body unchanged if serialization fails. Subsequent readers, retries and
mirrors use the final generation. There is no response-body XML API.

```rust
let envelope = xml.parse(req.body)
if envelope == nil { return deny(400, "Invalid XML") }
let body = xml.find(envelope, "urn:soap", "Body", 0)
let item = xml.find(body, "urn:example", "item", 0)
if item == nil { return deny(400, "Missing item") }
req.set_header("X-Item", xml.value(item))

let id = xml.attr("", "id", "7")
let value = xml.text("hello & goodbye")
let request = xml.element("urn:legacy", "request", {"id": id}, {"value": value})
xml.set_body(request)
req.set_header("Content-Type", "application/xml")
return next()
```

## Appendix: Function Quick Reference

| Function | Category | Return Type | Parameters |
|----------|----------|-------------|------------|
| `req.header(name)` | Request Reading | `Str\|Nil` | 1 |
| `req.method` | Request Reading | `Str` | 0 |
| `req.path` | Request Reading | `Str` | 0 |
| `req.query(name)` | Request Reading | `Str\|Nil` | 1 |
| `req.query_string` | Request Reading | `Str\|Nil` | 0 |
| `req.cookie(name)` | Request Reading | `Str\|Nil` | 1 |
| `req.direct_peer_ip` | Request Reading | `Str` | 0 |
| `req.remote_ip` | Request Reading | `Str` | 0 |
| `req.path_param(name)` | Request Reading | `Str\|Nil` | 1 |
| `req.header_names` | Request Reading | `List` | 0 |
| `req.scheme` | Request Reading | `Str` | 0 |
| `req.host` | Request Reading | `Str\|Nil` | 0 |
| `req.uri` | Request Reading | `Str` | 0 |
| `req.content_type` | Request Reading | `Str\|Nil` | 0 |
| `req.has_header(name)` | Request Reading | `Bool` | 1 |
| `req.protocol` | Request Metadata | `Str` | 0 |
| `req.grpc_service` | Request Metadata | `Str\|Nil` | 0 |
| `req.grpc_method` | Request Metadata | `Str\|Nil` | 0 |
| `req.content_length` | Request Metadata | `Int\|Nil` | 0 |
| `req.client_cert_cn` | Request Metadata | `Str\|Nil` | 0 |
| `req.client_cert_fingerprint` | Request Metadata | `Str\|Nil` | 0 |
| `req.set_header(name, value)` | Request Modification | `Nil` | 2 |
| `req.append_header(name, value)` | Request Modification | `Nil` | 2 |
| `req.remove_header(name)` | Request Modification | `Nil` | 1 |
| `req.set_uri(uri)` | Request Modification | `Nil` | 1 |
| `req.set_host(host)` | Request Modification | `Nil` | 1 |
| `req.set_method(method)` | Request Modification | `Nil` | 1 |
| `resp.status` | Response Reading | `Int\|Nil` | 0 |
| `resp.header(name)` | Response Reading | `Str\|Nil` | 1 |
| `resp.has_header(name)` | Response Reading | `Bool` | 1 |
| `resp.header_names` | Response Reading | `List` | 0 |
| `resp.content_type` | Response Reading | `Str\|Nil` | 0 |
| `resp.set_status(code)` | Response Modification | `Nil` | 1 |
| `resp.set_header(name, value)` | Response Modification | `Nil` | 2 |
| `resp.append_header(name, value)` | Response Modification | `Nil` | 2 |
| `resp.remove_header(name)` | Response Modification | `Nil` | 1 |
| `ctx.get(key)` | Context | `Str\|Nil` | 1 |
| `ctx.set(key, value)` | Context | `Nil` | 2 |
| `ctx.remove(key)` | Context | `Nil` | 1 |
| `len(s)` | String | `Int` | 1 |
| `substr(s, start, end)` | String | `Str` | 3 |
| `to_str(v)` | String | `Str` | 1 |
| `to_int(s)` | String | `Int\|Nil` | 1 |
| `to_upper(s)` | String | `Str` | 1 |
| `to_lower(s)` | String | `Str` | 1 |
| `trim(s)` | String | `Str` | 1 |
| `split(s, delimiter)` | String | `List` | 2 |
| `join(list, delimiter)` | String | `Str` | 2 |
| `base64_encode(s)` | Encoding | `Str` | 1 |
| `base64_decode(s)` | Encoding | `Str\|Nil` | 1 |
| `url_encode(s)` | Encoding | `Str` | 1 |
| `url_decode(s)` | Encoding | `Str` | 1 |
| `hex_encode(s)` | Encoding | `Str` | 1 |
| `hex_decode(s)` | Encoding | `Str\|Nil` | 1 |
| `sha256(s)` | Hash | `Str` | 1 |
| `md5(s)` | Hash | `Str` | 1 |
| `hmac_sha256(key, data)` | Hash | `Str` | 2 |
| `regex_find(s, pattern)` | Regex | `Str\|Nil` | 2 |
| `regex_replace(s, pattern, replacement)` | Regex | `Str` | 3 |
| `time_now()` | Time | `Int` | 0 |
| `time_format(timestamp, format)` | Time | `Str` | 2 |
| `json_get(json_str, path)` | JSON | `Str\|Int\|Bool\|Nil` | 2 |
| `req.body.json_select({name: path, ...})` | JSON | `Map` | 1 map |
| `http.call(method, url, body [, timeout_ms [, headers]])` | Network | `Map` | 3–5 |
| `ip_in_cidr(ip, cidr)` | Network | `Bool` | 2 |
| `uuid()` | Utility | `Str` | 0 |
| `range(start, end)` | Utility | `List` | 2 |
| `log(msg)` | Utility | `Nil` | 1 |
| `log_fmt(template_literal, value1, ...)` | Utility | `Nil` | 1–255 |
| `log_warn(msg)` | Utility | `Nil` | 1 |
| `log_error(msg)` | Utility | `Nil` | 1 |
| `xml.parse(source)` | Bounded XML | `Xml\|Nil` | 1 |
| `xml.kind(node)` | Bounded XML | `Str\|Nil` | 1 |
| `xml.name(node)` | Bounded XML | `Str\|Nil` | 1 |
| `xml.namespace(node)` | Bounded XML | `Str\|Nil` | 1 |
| `xml.value(node)` | Bounded XML | `Str\|Nil` | 1 |
| `xml.child_count(node)` | Bounded XML | `Int\|Nil` | 1 |
| `xml.child(node, index)` | Bounded XML | `Xml\|Nil` | 2 |
| `xml.find(node, namespace, name, occurrence)` | Bounded XML | `Xml\|Nil` | 4 |
| `xml.attribute_count(node)` | Bounded XML | `Int\|Nil` | 1 |
| `xml.attribute(node, index)` | Bounded XML | `Xml\|Nil` | 2 |
| `xml.attr_value(node, namespace, name)` | Bounded XML | `Str\|Nil` | 3 |
| `xml.text(value)` | Bounded XML | `Xml` | 1 |
| `xml.attr(namespace, name, value)` | Bounded XML | `Xml` | 3 |
| `xml.element(namespace, name, attributes, children)` | Bounded XML | `Xml` | 4 |
| `xml.stringify(node)` | Bounded XML | `Str\|Nil` | 1 |
| `xml.set_body(node)` | Bounded XML | `Nil` | 1 |

---

## Related Documentation

- [DSL Plugin Overview](./README.md)
- [Language Syntax Reference](./language-syntax.md)
- [Sandbox Security Model](./sandbox-security.md)
- [Practical Examples](./examples.md)
- [Bytecode Guide](./bytecode.md)

## Response header policy

`resp.set_header`, `resp.append_header`, and `resp.remove_header` check dynamic names against the shared policy and keep their existing error semantics.

[Response header policy](../../gateway-api/response-header-modifier.md#response-header-policy).

### Strict JSON reconstruction

These functions operate on explicitly encoded JSON strings. They do not treat an
ordinary string value as encoded JSON implicitly. Raw subtrees retain their
original numeric spelling, including integers above `u64` and precise decimals.
Duplicate decoded object keys are rejected recursively, including escaped aliases.

| Function | Result |
| --- | --- |
| `json.read(source)` | Validates a `Str` or complete request `Body`; returns the exact source string, or `nil` for malformed JSON, duplicate keys or invalid Body UTF-8 |
| `json.get_raw(document, key)` | Returns the encoded top-level member as `Str`; missing/non-object/malformed input returns `nil`; JSON null returns the string `"null"` |
| `json.get_string(document, key)` | Decodes a top-level JSON string; missing, non-string, non-object or malformed input returns `nil` |
| `json.keys(document)` | Returns decoded top-level member names as a `List`; non-object or malformed input returns `nil` |
| `json.set_raw(document, key, encoded_value)` | Inserts/replaces one top-level member with an explicitly validated JSON value; returns a complete JSON object string |
| `json.set_string(document, key, string)` | Inserts/replaces one top-level member with a safely escaped JSON string; returns a complete JSON object string |
| `json.set_body(document)` | Validates a complete JSON string, then atomically publishes it through the canonical request-body owner; returns `true` |

Keys are decoded names, not JSON Pointers. Construction requires an object and
rejects malformed input with a fixed `ApiError`. Incompatible arguments use
`TypeError`, except every `json.set_body` failure uses fixed request-body failure
HTTP 500 regardless of `errorPolicy`. String-only operations obey ordinary DSL
error policy for structural/work/string limits. Getters preserve missing versus
null; they never silently truncate a value.

Source and output are independently bounded by `min(64 KiB, maxStringLen)`, depth
32 and 4,096 nodes. `json.keys` also obeys `maxListLen`. Repeated parsing,
construction, escaped-key decoding and output copying consume one cumulative
execution work account. Body reads additionally consume the existing body scan
account and physical cap. Resource/scan/storage failures are fixed body failures;
malformed input remains `nil` so a script can issue its explicit 400.

`json.set_body` is request-stage only. It completes validation before one overwrite
and cannot modify a body frozen by an earlier security/GraphQL inspection. Place
reconstruction before all such inspectors, including Global/Gateway consumers.
It does not set framing headers; the Gateway owns Content-Length and
Transfer-Encoding. Set JSON Content-Type separately if the application requires it.

```text
let stored = json.read("{\"query\":\"query Q { a }\",\"operationName\":\"Q\"}")
let input = json.read(req.body)
if input == nil { return deny(400, "Invalid input") }
let variables = json.get_raw(input, "variables")
let envelope = stored
if variables != nil { envelope = json.set_raw(stored, "variables", variables) }
json.set_body(envelope)
req.set_header("Content-Type", "application/json")
```

This example demonstrates typed reconstruction only; it does not implement an
authorized registration endpoint or validate GraphQL syntax.

### Entry method and bounded Redis lookup

`req.request_method` returns the borrowed entry method captured before request
plugins rewrite the method, or `nil` if unavailable. Case is preserved. `req.method`
continues to return the live method. Admission depending on the original method
must compare the entry view before trusted rewrites.

`redis.get_bounded(alias, key, max_bytes)` returns the existing
`{ok, value, error}` map. A missing key succeeds with `value: nil`. The bound must
be a positive `Int` representable as `usize`; invalid bounds return
`invalid_argument` before provider lookup or I/O. The effective cap is
`min(max_bytes, maxStringLen, 65536)`; larger requested bounds are clamped.
An oversized value returns `result_too_large`, never a partial success.
Non-string or invalid UTF-8 replies return `invalid_response`. Typed provider
command timeouts return `timeout`; other command failures return
`dependency_error`. Errors are fixed classes, with the existing `e` fallback
when `maxStringLen` cannot hold the class.

The live LinkSys client is resolved on each call. This operation reuses its pool
and configured command timeout; there is no general VM wall-clock timeout.
Caller cancellation drops the await. Raw replies are checked before allocating
DSL text, but redis-rs has already aggregated the complete Redis reply: this is a
conversion bound, not a Redis transport memory cap. Existing `redis.get` retains
its documented truncation behavior. Publication can use existing
`redis.set(alias, key, value, 604800000)` for a seven-day TTL; reads do not renew it.
