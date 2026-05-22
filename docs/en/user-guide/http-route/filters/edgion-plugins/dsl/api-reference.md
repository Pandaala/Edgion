# EdgionDSL API Reference

This document lists all built-in functions in EdgionDSL, organized by category. Each function includes its full signature, parameter descriptions, return values, constraints, and usage examples.

**Type overview**: EdgionDSL has 6 runtime types: `Str` (string), `Int` (64-bit integer), `Bool` (boolean), `Nil` (null/missing), `List` (a list of strings; cannot be constructed directly by user code), and `Map` (an ordered string-keyed map; returned by `http.call()` and constructible via `{key: expr}` literals).

**Phase overview**: Some functions are only valid in specific execution phases. `req.*` write operations and `grpc_error()` are only meaningful in the request phase; `resp.*` read and write operations are only valid in the response phase.

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

Returns the HTTP method of the current request, such as `"GET"`, `"POST"`, `"PUT"`, `"DELETE"`, etc. Always returned in uppercase.

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

### req.client_ip

**Signature**: `req.client_ip` → `Str`

Returns the client IP address after processing by the `real-ip` plugin. If the `real-ip` plugin is not configured, this is identical to `req.remote_ip` and returns the direct peer address of the connection.

**Return value**: IP address string (IPv4 or IPv6 format)

**Example**:
```
let ip = req.client_ip
if ip_in_cidr(ip, "10.0.0.0/8") {
  req.set_header("X-Internal-Request", "true")
}
```

---

### req.remote_ip

**Signature**: `req.remote_ip` → `Str`

Returns the direct peer IP address of the TCP connection, without any proxy header processing. This is the actual remote address of the underlying connection.

**Return value**: IP address string (IPv4 or IPv6 format)

**Example**:
```
let remote = req.remote_ip
log("direct connection from: " + remote)
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

## Request Metadata (req.*)

### req.protocol

**Signature**: `req.protocol` → `Str`

Returns the protocol type identifier of the current request. Possible values are:
- `"http"` — Ordinary HTTP/1.1 or HTTP/2 request
- `"grpc"` — gRPC request (`Content-Type: application/grpc`)
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

Changes the HTTP method of the request. The method name is case-insensitive; it is converted to uppercase before being applied.

**Parameters**:
- `method` (Str) — New HTTP method, e.g. `"GET"`, `"POST"`

**Return value**: `nil`

**Example**:
```
if req.method == "HEAD" {
  req.set_method("GET")
}
```

---

## Response Reading (resp.*)

> **Note**: The following functions are only valid in the response phase (`upstreamResponseFilterPlugins`). When called in the request phase, read functions typically return `nil`, and the status code also returns `nil`.

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

## Response Modification (resp.*)

### resp.set_status

**Signature**: `resp.set_status(code)` → `Nil`

Modifies the response status code sent to the client. `code` must be an integer between 100 and 599; otherwise a runtime error is raised.

**Parameters**:
- `code` (Int) — HTTP status code, in the range 100–599

**Return value**: `nil`

**Constraints**: `code` must be an integer type and within the range 100–599; a runtime error is raised if it is out of range.

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

Context variables are shared across plugins within the same request lifecycle. Values set in the request phase are readable in the response phase and in access logs.

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

Sets a key-value pair in the current request context. The value is visible to subsequent plugins and access logs for the same request. `value` is stored as a string.

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

Converts any type to a string:
- `Str` → original value
- `Int` → decimal representation
- `Bool` → `"true"` or `"false"`
- `Nil` → `"nil"`
- `List` → `"[list]"`

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

Decodes a URL-encoded string. `%XX` sequences are restored to their corresponding characters, and `+` is restored to a space (compatible with `application/x-www-form-urlencoded`). Invalid `%XX` sequences are preserved as-is.

**Parameters**:
- `s` (Str) — URL-encoded string

**Return value**: Decoded string

**Example**:
```
let raw = req.query("callback")
if raw != nil {
  let decoded = url_decode(raw)
  req.set_header("X-Callback-Url", decoded)
}
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

Computes the MD5 hash of a string, returning a 32-character lowercase hexadecimal digest. Note that MD5 is not suitable for security-sensitive use cases (such as password storage); it should only be used for non-security purposes such as data integrity checks or cache key generation.

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

Computes a message authentication code using the HMAC-SHA256 algorithm, returning a 64-character lowercase hexadecimal string. Commonly used to verify request signatures.

**Parameters**:
- `key` (Str) — Secret key string
- `data` (Str) — Data to sign

**Return value**: 64-character hexadecimal HMAC digest

**Example**:
```
let secret = "my-secret-key"
let timestamp = req.header("X-Timestamp")
let signature = req.header("X-Signature")
if timestamp == nil or signature == nil {
  return deny(401, "missing signature headers")
}
let expected = hmac_sha256(secret, timestamp + ":" + req.path)
if signature != expected {
  return deny(401, "invalid signature")
}
```

---

## Regular Expressions

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

**Signature**: `json_get(json_str, path)` → `Str | Int | Bool | Nil`

Extracts a value from a JSON string using a dot-separated path. The path format is `"field"` or `"parent.child.grandchild"`; numeric indices can be used for arrays, e.g. `"items.0.name"`. Returns `nil` if the path does not exist or JSON parsing fails.

**Parameters**:
- `json_str` (Str) — JSON string
- `path` (Str) — Dot-separated field path

**Return value**: Extracted value (`Str`, `Int`, `Bool`, or `Nil`)

**Constraints**:
- The JSON string length must not exceed `maxStringLen` (default 8192 bytes)
- The path supports at most 32 segments; a runtime error is triggered if this is exceeded
- JSON numbers that can be represented as `i64` are returned as `Int`; otherwise they are returned as `Str`; array and object nodes are returned as their JSON string representation

**Example**:
```
let payload = req.header("X-Jwt-Claims")
if payload != nil {
  let user_id = json_get(payload, "sub")
  let role = json_get(payload, "role")
  if user_id != nil {
    req.set_header("X-User-Id", to_str(user_id))
  }
  if role == "admin" {
    req.set_header("X-Is-Admin", "true")
  }
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
- `body` (Str | Nil) — Request body; pass `nil` for no body
- `timeout_ms` (Int, optional) — Per-request timeout in milliseconds; capped to `httpMaxTimeoutMs` (default 5000)
- `headers` (Map, optional) — Additional request headers as a `Map` of string key-value pairs

**Return value**: A `Map` with the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `status` | Int | HTTP response status code; `0` if the request failed (timeout, connection error, SSRF block, etc.) |
| `body` | Str | Response body as UTF-8 text; empty string if the call failed or the body is empty |
| `headers` | Map | Response headers; each key is the lowercase header name, each value is the last header value for that name |

**This function never raises a runtime error.** Network failures, timeouts, and SSRF blocks all return `status = 0` with an empty body.

**Constraints**:
- Only `http` and `https` URL schemes are accepted; other schemes (e.g. `file://`, `ftp://`) return `status = 0`.
- When `httpBlockPrivateIps: true`, requests to loopback, private, and link-local addresses return `status = 0`.
- The response body is truncated at `httpMaxResponseBodyBytes` (default 1 MiB).
- The response `headers` Map is silently truncated to the first `max_map_size` entries (SSRF / large-response-header guard); the call still succeeds.
- The actual timeout applied is `min(timeout_ms, httpMaxTimeoutMs)`.
- At most 10 redirects are followed (global client setting).

**Configuration fields** (inside `config`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `httpMaxTimeoutMs` | Integer | `5000` | Hard cap on per-call timeout (ms) |
| `httpMaxResponseBodyBytes` | Integer | `1048576` | Maximum response body size (bytes); body is truncated if exceeded |
| `httpBlockPrivateIps` | Boolean | `false` | Block outbound calls to private/loopback/link-local IP addresses (SSRF guard) |

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
- `ok: false` — A Redis connection or operation error occurred; `value` is `nil`; `error` holds the error message.
- When the `link` parameter does not match any alias in `linkSysRefs`, returns `ok: false, error: "link 'x' not found in linkSysRefs"`.
- When the specified link exists but is not yet ready, returns `ok: false, error: "link 'x' not ready"`.

**Configuration field** (inside `config`):

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `linkSysRefs` | Vec<{namespace, name}> | Required when `redis.*` is used | List of LinkSys Redis resource references. Each element has `namespace` and `name` fields; the `name` is the alias used in `redis.*` calls. Configuring this field is mandatory if any `redis.*` builtin appears in the script; a missing `linkSysRefs` causes the plugin to fail at load time. |

### redis.get

**Signature**: `redis.get(link, key)` → `Map { ok: Bool, value: Str | Nil, error: Str | Nil }`

Reads the value of a string key. Returns `value: nil` when the key does not exist.

**Parameters**:
- `link` (Str) — Alias name from `linkSysRefs[].name`
- `key` (Str) — Redis key name

**Example — fail-close**:
```
let r = redis.get("cache", "feature:dark-mode")
if !r["ok"] {
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
if !r["ok"] {
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

**Example — simple rate limiting**:
```
let r = redis.incr("cache", "rate:" + req.client_ip)
if !r["ok"] {
  log_warn("redis error: " + r["error"])
  return deny(503, "service unavailable")
}
if r["value"] > 100 {
  return deny(429, "rate limit exceeded")
}
```

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

Gets all fields and values of a hash. Returns an empty `Map` when the key does not exist. `ok: false` is returned on a connection error, or when the hash contains more entries than the `max_map_size` limit (in which case the oversize `Map` is not constructed and `error` describes the actual size and limit).

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
let ip = req.client_ip
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
- `status` (Int) — HTTP status code, in the range 100–599
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

**Signature**: `grpc_error(code, message)` — terminates the request (request phase only)

Returns a standard gRPC error to the client, sending `grpc-status` and `grpc-message` via HTTP/2 trailers. `code` is a gRPC status code in the range 0–16; values outside this range are automatically downgraded to 13 (INTERNAL).

**Parameters**:
- `code` (Int) — gRPC status code (0–16)
- `message` (Str | Nil) — Error message; pass `nil` for no message

**Constraints**: Only valid in the request phase (`requestPlugins`). Calling this in the response phase degrades to `terminate()`, because the response headers have already been committed and HTTP/2 trailers can no longer be appended.

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

Terminates request processing immediately without sending any response to the client (the connection is closed). Typically not used directly; it is primarily the fallback behavior of `grpc_error()` when called in the response phase.

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
  log_error("missing signature header, rejecting request from: " + req.client_ip)
  return deny(401, "signature required")
}
```

---

## Appendix: Function Quick Reference

| Function | Category | Return Type | Parameters |
|----------|----------|-------------|------------|
| `req.header(name)` | Request Reading | `Str\|Nil` | 1 |
| `req.method` | Request Reading | `Str` | 0 |
| `req.path` | Request Reading | `Str` | 0 |
| `req.query(name)` | Request Reading | `Str\|Nil` | 1 |
| `req.query_string` | Request Reading | `Str\|Nil` | 0 |
| `req.cookie(name)` | Request Reading | `Str\|Nil` | 1 |
| `req.client_ip` | Request Reading | `Str` | 0 |
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
| `http.call(method, url, body [, timeout_ms [, headers]])` | Network | `Map` | 2–5 |
| `ip_in_cidr(ip, cidr)` | Network | `Bool` | 2 |
| `uuid()` | Utility | `Str` | 0 |
| `range(start, end)` | Utility | `List` | 2 |
| `log(msg)` | Utility | `Nil` | 1 |
| `log_warn(msg)` | Utility | `Nil` | 1 |
| `log_error(msg)` | Utility | `Nil` | 1 |

---

## Related Documentation

- [DSL Plugin Overview](./README.md)
- [Language Syntax Reference](./language-syntax.md)
- [Sandbox Security Model](./sandbox-security.md)
- [Practical Examples](./examples.md)
- [Bytecode Guide](./bytecode.md)
