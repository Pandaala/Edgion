# EdgionDSL Language Syntax Reference

This document describes the complete syntax specification for EdgionDSL. EdgionDSL is a lightweight scripting language designed specifically for Edgion gateway plugins. It runs inside a secure sandbox and is used to implement custom request/response processing logic.

---

## 1. Type System

EdgionDSL has seven built-in value types:

| Type | Description | Example Literals |
|------|-------------|-----------------|
| `String` | UTF-8 string | `"hello"` |
| `Int` | 64-bit signed integer | `42`, `-1` |
| `Bool` | Boolean value | `true`, `false` |
| `Nil` | Null value, representing "no value" | `nil` |
| `Body` | Opaque handle for the current proxied request body | Returned by `req.body` |
| `List` | A read-only list of strings used for iteration | Returned by APIs such as `req.header_names()` |
| `Map` | A key-value map with `String` keys and any-type values | `{"key": "value"}`, `{"status": 200}` |

The `List` type is used exclusively as an iteration container. It cannot be constructed from a literal and cannot be directly compared to other types.

The `Map` type holds an ordered set of key-value pairs. Keys are always `String`; values can be any DSL type including nested `Map`. Map values are read via index access (`map["key"]`). Maps are currently returned by `http.call()` and cannot be constructed with arbitrary nesting in literals, but a flat map literal `{key: expr, ...}` is supported.

### 1.1 Type Conversion

When calling `to_str(val)`, or when implicit conversion is triggered during string concatenation, each type behaves as follows:

| Source Type | Conversion Result |
|-------------|-------------------|
| `Int` | Decimal string, e.g. `42` → `"42"`, `-1` → `"-1"` |
| `Bool` | `true` → `"true"`, `false` → `"false"` |
| `Nil` | `"nil"` |
| `List` | `"[list]"` (fixed string, contents not expanded) |
| `Map` | `"[map]"` (fixed string, contents not expanded) |
| `String` | Unchanged |
| `Body` | **Rejected**; use a bounded Body read explicitly |

```dsl
let n = 100
let s = to_str(n)      // s == "100"

let b = false
let t = to_str(b)      // t == "false"

let x = nil
let y = to_str(x)      // y == "nil"
```

### 1.2 Truthiness Rules

EdgionDSL uses Lua-style truthiness semantics:

- **Falsy**: `Nil` and `false`
- **Truthy**: all other values, including `0`, the empty string `""`, empty lists, and `Body`

```dsl
// The following conditions are all truthy (the if branch is entered)
if 0 { ... }
if "" { ... }

// The following conditions are all falsy (the if branch is not entered)
if nil { ... }
if false { ... }
```

> Note: The empty string `""` is truthy in EdgionDSL, unlike languages such as Python. To check whether a string is empty, use `len(s) == 0`.

### 1.3 Cross-Type Equality

Values of different types are normally not equal, even if their contents
appear the same. `Body`/`String` is the one exception: it performs streaming
logical-text equality in either operand order. Two Body values are equal
because both identify the current request body.

```dsl
1 == "1"       // false, Int != String
nil == false   // false, Nil != Bool
0 == false     // false, Int != Bool
req.body == "expected"  // streaming Body/String equality
```

---

## 2. Variables

### 2.1 Variable Declaration

EdgionDSL uses the `let` keyword to declare variables. Variables are immutable by default; add the `mut` modifier to declare a mutable variable:

```dsl
// Immutable variables
let name = "alice"
let code = 200

// Mutable variables
let mut counter = 0
let mut message = "init"
```

### 2.2 Assignment

Only variables declared with `let mut` can be reassigned. Assigning to an immutable variable produces a compile-time error:

```dsl
let x = 10
x = 20          // Compile error: x is immutable

let mut y = 10
y = 20          // Valid
y = y + 5      // y == 25
```

### 2.3 Scope

A variable's scope begins at the point of declaration and ends at the closing brace (`}`) of the enclosing block. Inner blocks can access variables from outer blocks, but outer blocks cannot access variables declared in inner blocks:

```dsl
let token = req.header("Authorization")

if token != nil {
    let mut prefix = "Bearer "
    // Both token and prefix are accessible here
    if token.starts_with(prefix) {
        let cleaned = substr(token, 7, len(token))
        // token, prefix, and cleaned are all accessible here
    }
    // cleaned is not accessible here
}
// prefix is not accessible here
```

---

## 3. Operators

### 3.1 Arithmetic Operators

| Operator | Description | Example |
|----------|-------------|---------|
| `+` | Addition (Int) or string concatenation (String) | `1 + 2`, `"a" + "b"` |
| `-` | Subtraction | `10 - 3` |
| `*` | Multiplication | `4 * 5` |
| `/` | Integer division (truncated toward zero) | `7 / 2 == 3` |

**Integer division truncation**: Results are truncated toward zero, not rounded:

```dsl
7 / 2    // 3 (not 3.5)
-7 / 2   // -3 (not -4)
```

**Division by zero**: Produces a runtime `DivisionByZero` error. The script's subsequent behavior is determined by the `errorPolicy` configuration.

**String concatenation**: When either operand of `+` is a `String`, the other
operand is automatically converted to its textual representation before concatenation.
`Body` is intentionally excluded; use `read(offset, max_bytes)` explicitly:

```dsl
"count: " + 42           // "count: 42"
"flag: " + true          // "flag: true"
"value: " + nil          // "value: nil"
"hello" + " " + "world"  // "hello world"
```

### 3.2 Comparison Operators

| Operator | Description |
|----------|-------------|
| `==` | Equal |
| `!=` | Not equal |
| `<` | Less than |
| `>` | Greater than |
| `<=` | Less than or equal |
| `>=` | Greater than or equal |

Ordering comparisons (`<`, `>`, `<=`, `>=`) accept matching `Int`/`Int` or
`String`/`String` operands. A Body or mismatched pair produces a runtime type
error.

String equality compares exact bytes; String ordering is lexicographic:

```dsl
"abc" == "abc"   // true
"abc" != "def"   // true
100 > 50         // true
100 < 200        // true
```

### 3.3 Logical Operators

| Operator | Description |
|----------|-------------|
| `&&` | Logical AND (short-circuit) |
| `\|\|` | Logical OR (short-circuit) |
| `!` | Logical NOT (unary) |

**Short-circuit evaluation**:

- `&&`: if the left operand is falsy, the right operand is not evaluated
- `||`: if the left operand is truthy, the right operand is not evaluated

**Result type**: `&&` and `||` always evaluate to a `Bool` (`true` / `false`), regardless of the operand types. They are logical operators, not value-selection / null-coalescing operators — `false || "x"` yields `true`, not `"x"`. To select a value based on a condition, use an `if`/`else` block.

```dsl
let h = req.header("X-Token")

// Short-circuit: when h == nil is true, h.starts_with() is not called
if h == nil || h.starts_with("Bearer ") {
    return deny(401, "invalid token format")
}
```

### 3.4 Operator Precedence

Listed from highest to lowest:

| Precedence | Operators | Description |
|------------|-----------|-------------|
| 1 (highest) | `!`, `-` (unary) | Logical NOT, negation |
| 2 | `*`, `/` | Multiplication, division |
| 3 | `+`, `-` | Addition, subtraction |
| 4 | `<`, `>`, `<=`, `>=` | Ordering comparisons |
| 5 | `==`, `!=` | Equality comparisons |
| 6 | `&&` | Logical AND |
| 7 (lowest) | `\|\|` | Logical OR |

Use parentheses to explicitly control evaluation order:

```dsl
// Equivalent to: a || (b && c)
let result = a || b && c    // && binds tighter than ||, so b && c is evaluated first

// Use parentheses to change the order
let result2 = (a || b) && c
```

---

## 4. Control Flow

### 4.1 if / else if / else

Conditional branching: the execution path is chosen based on the truthiness of each condition:

```dsl
if condition {
    // executed when condition is truthy
} else if other_condition {
    // executed when other_condition is truthy
} else {
    // executed when all conditions are falsy
}
```

Full example:

```dsl
let method = req.method()

if method == "GET" {
    req.set_header("X-Method-Type", "read")
} else if method == "POST" || method == "PUT" {
    req.set_header("X-Method-Type", "write")
} else if method == "DELETE" {
    let admin = req.header("X-Admin-Token")
    if admin == nil {
        return deny(403, "DELETE requires admin token")
    }
    req.set_header("X-Method-Type", "delete")
} else {
    return deny(405, "method not allowed")
}
```

Condition expressions do not require parentheses, but block bodies must be enclosed in braces `{}`.

### 4.2 for...in (Iteration Loop)

Iterates over each element in a `List`. A `List` is returned by API calls such as `req.header_names()`, `split()`, or `range()`:

```dsl
for name in req.header_names() {
    // name is the current header name (type String)
    if name.starts_with("x-internal-") {
        req.remove_header(name)
    }
}
```

Iterating over a `split()` result:

```dsl
let path = req.path()
let segments = split(path, "/")

for seg in segments {
    if seg == "admin" {
        return deny(403, "access denied")
    }
}
```

### 4.3 for...in range() (Range Loop)

`range(start, end)` generates a sequence of integers from `start` (inclusive) to `end` (exclusive), returned as strings.

> Note: Each element produced by `range()` is a string (`"0"`, `"1"`, ...), not an integer.

```dsl
// Iterate from 0 to 9
for i in range(0, 10) {
    // i is the string "0", "1", ..., "9"
    let header_name = "X-Retry-" + i
    req.remove_header(header_name)
}
```

`range(start, end)` generates at most `min(max_list_len, 1000)` elements (where `max_list_len` is the per-plugin VM limit, default 1024). Any elements beyond that cap are silently truncated.

### 4.4 while (Conditional Loop)

Repeatedly executes the loop body while the condition is truthy:

```dsl
let mut i = 0
let mut total = 0

while i < 10 {
    total = total + i
    i = i + 1
}
// total == 45
```

The `while` loop is also subject to the `maxLoopIterations` configuration limit; exceeding it produces a runtime error.

### 4.5 break and continue

- `break`: immediately exits the current loop
- `continue`: skips the current iteration and proceeds to the next one

```dsl
let mut found = false

for name in req.header_names() {
    if name == "x-target-header" {
        found = true
        break   // exit the loop immediately once found
    }
}

// continue example
for name in req.header_names() {
    if !name.starts_with("x-forward-") {
        continue   // skip non-matching headers
    }
    req.remove_header(name)
}
```

`break` and `continue` only affect the innermost enclosing loop; multi-level jumps are not supported.

---

## 5. Text and Body Methods

String and Body values share the following method calls using dot notation.
Predicate methods preserve the existing nil-friendly `false` result when the
receiver or argument has the wrong type:

| Method | Parameter | Return Type | Description |
|--------|-----------|-------------|-------------|
| `.len()` | none | Int | Unicode scalar count for String or lossy-decoded Body text |
| `.is_empty()` | none | Bool | Whether String or Body text is empty |
| `.starts_with(prefix)` | String | Bool | Whether the string starts with the given prefix |
| `.ends_with(suffix)` | String | Bool | Whether the string ends with the given suffix |
| `.contains(substring)` | String | Bool | Whether the string contains the given substring |
| `.matches(pattern)` | String | Bool | Whether the string matches the given regular expression (validated at compile time) |

Body additionally provides `.byte_len()`, `.read(offset, max_bytes)`,
`.sha256()`, `.md5()`, `.hmac_sha256(key)`,
`.json_get("definite path")`, and the static
multi-path `.json_select({"name": "$.path"})`. Request-stage scripts may
mutate it with `.clear()`, `.append(s)`, `.prepend(s)`, `.remove(start, end)`,
and literal `.replace(from, to)`. See the [API
reference](./api-reference.md#reqbody) for bounds and failure behavior.

```dsl
let ua = req.header("User-Agent")

if ua != nil {
    if ua.starts_with("Mozilla") {
        req.set_header("X-Client-Browser", "true")
    }

    if ua.ends_with("Mobile Safari") {
        req.set_header("X-Client-Mobile", "true")
    }

    if ua.contains("Bot") {
        return deny(403, "bots not allowed")
    }
}
```

The regex pattern passed to `.matches()` is validated at script compile time.
It must also fit the bounded streaming DFA dialect used for both String and
Body. Unicode word boundaries are rejected; use `(?-u:\b)` for an ASCII word
boundary. An invalid or incompatible pattern causes the script to fail loading:

```dsl
let path = req.path()

// Regex match: check whether the path matches /api/v{number}/ format
if path.matches("^/api/v[0-9]+/") {
    req.set_header("X-Versioned-Api", "true")
}
```

---

## 6. Return Statements

A script must end explicitly with a `return` statement, or implicitly at the end of the script body.

### 6.1 return next()

Passes the request through, continuing to the next plugin. This is the most common return form:

```dsl
// Add a trace header and continue
req.set_header("X-Trace-Id", uuid())
return next()
```

### 6.2 return deny(status, body)

Rejects the request with the specified HTTP status code and response body:

- `status`: integer, valid final-response range 200–599
- `body`: string or `nil` (nil means no response body)

```dsl
let token = req.header("Authorization")
if token == nil {
    return deny(401, "Authorization header is required")
}

// No response body
return deny(204, nil)
```

### 6.3 return terminate()

Immediately terminates request processing without sending any response. Use this when you need to close the connection directly:

```dsl
// Deliberately match the physical peer for this exact network-origin block.
let ip = req.direct_peer_ip()
if ip == "1.2.3.4" {
    return terminate()
}
```

### 6.4 return grpc_error(code, message)

Returns a native gRPC error, delivering a standard gRPC status code via HTTP/2 trailers:

- `code`: integer, valid range 0–16 (values out of range are clamped to 13, i.e. INTERNAL)
- `message`: a human-readable error description string

```dsl
if req.protocol != "grpc" {
    return next()
}

let token = req.header("authorization")
if token == nil {
    return grpc_error(16, "missing authorization token")  // UNAUTHENTICATED
}

let service = req.grpc_service
if service == "internal.AdminService" {
    return grpc_error(7, "permission denied")  // PERMISSION_DENIED
}
```

> Supported in both `requestPlugins` and `upstreamResponseFilterPlugins`. The request phase writes a direct trailers-only response; the response phase aborts the upstream response and preserves the exact code and optional message for the canonical terminal writer. Wire HTTP status remains `200`; `grpc_status` records the independent gRPC application status. See [DSL Plugin — gRPC Error Returns](./README.md#grpc-error-returns).

### 6.5 Implicit Return

If script execution reaches the end without an explicit `return` statement, the default behavior is equivalent to `return next()`:

```dsl
// No return statement — equivalent to return next()
let ua = req.header("User-Agent")
if ua != nil {
    req.set_header("X-Has-UA", "true")
}
// Script ends, next() is called automatically
```

---

## 7. String Literals

Strings are delimited by double quotes `"`:

```dsl
let greeting = "hello, world"
let empty = ""
```

The following escape sequences are supported:

| Escape Sequence | Meaning |
|-----------------|---------|
| `\n` | Newline (LF) |
| `\t` | Tab |
| `\\` | Backslash `\` |
| `\"` | Double quote `"` |

```dsl
let multiline_msg = "line one\nline two"
let path_sep = "C:\\Users\\alice"
let quoted = "she said \"hello\""
```

Raw string literals and multi-line literal syntax are not supported. For complex content, use escape sequences or string concatenation.

---

## 8. Map Literals and Index Access

### 8.1 Map Literals

A flat map (string keys, any-type values) can be constructed with a `{key: expr, ...}` literal. Keys must be string literals; values can be any expression:

```dsl
let hdrs = {"Content-Type": "application/json", "X-Api-Key": "secret"}
let meta = {"retries": 3, "enabled": true}
```

Map literals are primarily used to pass the optional `headers` argument to `http.call()`.

### 8.2 Index Access

Use `expr[key]` to read a value from a `Map` or a `List`:

- **Map**: `key` must be a `String` expression. Returns `nil` if the key is absent.
- **List**: index access on `List` is not supported directly — iterate with `for...in` instead.

```dsl
let r = http.call("GET", "https://api.example.com/status", nil)

// Read a top-level field
let code = r["status"]          // Int

// Read a nested map field (response headers are returned as a Map)
let ct = r["headers"]["content-type"]   // Str | Nil

if code == 200 {
  req.set_header("X-Backend-Ok", "true")
}
```

`len()` works on `Map` and returns the number of entries:

```dsl
let m = {"a": 1, "b": 2, "c": 3}
let n = len(m)    // 3
```

---

## 9. Comments

EdgionDSL supports single-line comments, starting with `//` and extending to the end of the line:

```dsl
// This is a single-line comment

let token = req.header("Authorization")  // Retrieve the Authorization header

// Multi-step logic can be annotated in segments
// Step 1: validate required fields
if token == nil {
    return deny(401, "missing token")
}

// Step 2: pass the request through
return next()
```

Block comments (`/* ... */`) are not supported.

---

## 10. Complete Examples

### Example 1: Multi-Condition Request Validation

```dsl
// Validate method, token, and path prefix
let method = req.method()
let token = req.header("X-Api-Key")
let path = req.path()

if token == nil {
    return deny(401, "X-Api-Key header is required")
}

if method == "DELETE" && !path.starts_with("/api/v2/") {
    return deny(403, "DELETE is only allowed on /api/v2/ resources")
}

// Record the protocol type
let proto = req.protocol
if proto == "grpc" {
    req.set_header("X-Protocol", "grpc")
} else {
    req.set_header("X-Protocol", "http")
}

return next()
```

### Example 2: Iterate and Strip Internal Headers

```dsl
// Remove all x-internal-* request headers to prevent clients from injecting internal routing information
for name in req.header_names() {
    if name.starts_with("x-internal-") {
        req.remove_header(name)
    }
}

return next()
```

### Example 3: Loop with a Counter

```dsl
// Count headers starting with "x-tag-" and reject the request if there are more than 10
let mut tag_count = 0

for name in req.header_names() {
    if name.starts_with("x-tag-") {
        tag_count = tag_count + 1
    }
    if tag_count > 10 {
        break
    }
}

if tag_count > 10 {
    return deny(400, "too many x-tag-* headers")
}

return next()
```

### Example 4: gRPC Service Routing Control

```dsl
// Allow only specific gRPC services to pass through
if req.protocol != "grpc" {
    return next()
}

let service = req.grpc_service

if service == nil {
    return grpc_error(3, "cannot determine gRPC service name")  // INVALID_ARGUMENT
}

// Allowlist check
let allowed = false
if service == "user.UserService" {
    allowed = true
}
if service == "order.OrderService" {
    allowed = true
}

if !allowed {
    return grpc_error(12, "service not available: " + service)  // UNIMPLEMENTED
}

return next()
```

### Example 5: IP Range Check

```dsl
// Deliberately use the physical peer for this internal-origin bypass.
let ip = req.direct_peer_ip()

// Internal IPs are allowed through directly without a token
if ip_in_cidr(ip, "10.0.0.0/8") || ip_in_cidr(ip, "172.16.0.0/12") {
    req.set_header("X-Internal-Request", "true")
    return next()
}

// External requests must carry a token
let token = req.header("Authorization")
if token == nil {
    return deny(401, "Authorization required for external requests")
}

return next()
```

---

## Related Documentation

- [DSL Plugin Configuration Reference](./README.md) — configuration parameters, API reference, error policies
- [Filters Overview](../../overview.md)
- [Plugin Composition and References](../../plugin-composition.md)
