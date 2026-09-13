# Sandbox Security Model

This document describes the security mechanisms of the EdgionDSL script execution environment, including resource limits, error handling policies, and API availability per execution phase.

## Security Design Philosophy

The EdgionDSL sandbox is built on the following core principles:

**Minimal-capability by default**: Scripts start with no filesystem access, no environment variable reads, and no process management. All interactions with the gateway context (reading request headers, rejecting requests, etc.) must go through audited built-in functions whose behavior is well-defined and bounded. Outbound HTTP access uses `http.call()`, which enforces timeouts, response size limits, and an optional SSRF guard (`httpBlockPrivateIps`). Redis access uses `redis.*` through explicitly configured LinkSys references.

**Defense in depth**: Security is enforced at two layers. Compile-time validation statically checks structural constraints — instruction count, variable count, constant pool size, and so on — when a script is loaded, rejecting scripts that are overly complex or maliciously crafted. Runtime limits dynamically track execution steps, loop iterations, and function call counts on every request, preventing scripts from exceeding expected resource consumption at runtime. The two layers are complementary; a failure in one does not compromise the other.

**Panic isolation**: `execute_safe` catches unwinding panics across asynchronous VM execution, discards the panic payload, records only the fixed `vm panic` classification, and applies the configured `errorPolicy`.

**Observability**: Runtime failures use fixed classifications in `PluginLog`, carried by the request access log. The VM failure entry does not include panic payloads, script names, or triggering values. For example, API failures record `api error`, and request-body hard failures record `request body access error`.

## Runtime Resource Limits

The following limits are tracked independently on each request and do not interfere with one another. All configuration fields are located inside the plugin's `config` block.

| Limit | Config Field | Default | Min | Max | Description |
|-------|-------------|---------|-----|-----|-------------|
| Max execution steps | `maxSteps` | 10,000 | 1 | 1,000,000 | Total bytecode instructions per execution |
| Max loop iterations | `maxLoopIterations` | 100 | 1 | 100,000 | Maximum iterations for a single loop body |
| Max function calls | `maxCallCount` | 500 | 1 | 100,000 | Total number of built-in function calls |
| Max stack depth | `maxStackDepth` | 128 | 1 | 1,024 | Value stack depth |
| Max string length | `maxStringLen` | 8,192 | 1 | 1,048,576 | Maximum runtime string bytes and requested Body.read bytes per call |
| Max body scan passes | `maxBodyScanPasses` | 4 | 1 | 16 | Cumulative raw bytes scanned, measured against the resolved physical request-body cap |

### Limit Details

**maxSteps (Maximum Execution Steps)**

Each bytecode instruction consumes one step. Steps are the most direct measure of a script's overall computational cost and guard against infinite loops and excessive computation. A typical request-processing script — reading a few headers, doing conditional checks, setting a header — usually consumes no more than a few dozen steps. The default of 10,000 is sufficient for the vast majority of use cases. When the step budget is exhausted, the VM raises a `StepLimitExceeded` error.

**maxLoopIterations (Maximum Loop Iterations)**

Caps the number of iterations for a single `for` loop. For example, when iterating over a list, if the number of elements exceeds this value the loop terminates early and raises a `LoopLimitExceeded` error. This limit works in tandem with `maxSteps`: even if the step budget has not been exhausted, a loop with an excessive number of iterations will still be intercepted.

**maxCallCount (Maximum Function Calls)**

Counts the total number of built-in function invocations throughout the entire execution, including `req.header()`, `resp.set_header()`, `json_get()`, and all other built-in functions. This limit prevents scripts from producing an amplification effect through a large number of function calls — for example, repeatedly invoking expensive APIs inside a loop.

**maxStackDepth (Maximum Stack Depth)**

The VM uses a stack to evaluate expressions. Deeply nested expressions — such as multiple layers of composed function calls — consume more stack space. This limit prevents maliciously crafted deep nesting from exhausting memory. For typical business scripts, 128 levels of stack depth far exceeds any practical requirement.

**maxStringLen (Maximum String Length)**

Limits the maximum byte length of strings produced by concatenation operations. If the concatenated string exceeds this value, the VM raises a `StringTooLong` error. This limit also acts as an indirect constraint on functions that process string data, such as `json_get()`. Note that this limit applies to strings produced at runtime and does not affect string literals in the compile-time constant pool, which are governed by a separate compile-time limit.

It does not cap the opaque Body itself. It does cap strings returned by
`Body.read` and selected or serialized JSON values.
Each `Body.read(offset, max_bytes)` also requires `max_bytes <= maxStringLen`
(default 8 KiB, configurable up to 1 MiB). Oversized arguments raise
`StringTooLong` before scan reservation or EOF clipping, without silent clamping.
Body text reads check the limit during lossy UTF-8 decoding, before admitting
each output append or capacity request. Invalid UTF-8 expansion counts toward
the limit; overflow returns no prefix. The limit is not the total operation
memory ceiling: bounded scan buffers and temporary string conversion also
consume memory.

**maxBodyScanPasses (Cumulative Request-Body Scans)**

The VM gives one script execution a cumulative scan budget equal to the
resolved gateway physical request-body cap multiplied by
`maxBodyScanPasses`. Metadata-only `byte_len()` consumes no scan bytes. Scans
fetch source data in chunks of up to 64 KiB and charge every fetched chunk. An
early-exit predicate or overflowing text read may therefore charge the complete
final chunk even when it stops near that chunk's beginning. Unfetched bytes are
refunded when a scan returns a result, including a text-overflow result. A scan
that fails without returning accounting conservatively retains its reservation.
The same fetched-byte rule applies to memory, spill-file, and mutation-segment
sources; different segment boundaries can yield different early-stop charges.

`req.body.sha256()` lazily caches only the 32-byte digest on the current
immutable Body generation. Its first successful call scans normally; repeated
calls for the same generation consume zero scan bytes. Any successful Body
mutation publishes a new generation with an empty digest cache. Failed scans
and failed mutations never populate a new cache entry.

`req.body.md5()` and `req.body.hmac_sha256(key)` are intentionally uncached and
consume actual scan bytes on every call. MD5 is legacy interoperability only,
including in FIPS builds. Body HMAC adds no persistent key/MAC cache and is a
generation API, not an inbound verification API: normal DSL equality is not
constant-time.

### Configuration Example

```yaml
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "my-script"
        source: |
          # script content
        maxSteps: 5000
        maxLoopIterations: 50
        maxCallCount: 200
        maxStackDepth: 64
        maxStringLen: 4096
        maxBodyScanPasses: 4
        errorPolicy: deny
```

Using the default values is recommended for production. Only raise a specific limit when a script genuinely needs to handle larger data volumes or more complex logic — do not blindly increase all limits at once.

## Compile-Time Validation

When a script is loaded — compiled from `source` or validated from `bytecode` during configuration loading — compile-time validation is performed. The following limits are checked statically at compile time; a script that fails any of them is rejected and will not be loaded.

| Limit | Default | Description |
|-------|---------|-------------|
| Max instruction count | 1,000 | Total bytecode instructions after compilation |
| Max local variables | 64 | Number of local variable slots |
| Max constants | 256 | Number of constant pool entries |
| Single string constant length | 4,096 bytes | Maximum length of a single string literal |
| Total constant pool size | 32,768 bytes | Total bytes of all strings in the constant pool |
| Max loop nesting depth | 8 | Maximum nesting level of `for` loops |
| Regex pattern length | 1,024 bytes | Maximum length of a single regex literal |

Compile-time validation is a one-time cost. Once a script passes validation, the compiled result is cached and reused for subsequent requests without recompilation.

Regex patterns are not only checked for length at compile time — their syntax is also validated. An invalid regex causes a compile-time error when the script is loaded, rather than failing silently at runtime. All regex functions (`.matches()`, `regex_find()`, and `regex_replace()`) require string literal patterns; dynamic (variable) patterns are rejected at compile time. Patterns are pre-compiled and cached at compile time, eliminating per-request regex compilation overhead.

## Error Policy (errorPolicy)

`errorPolicy` controls gateway behavior when script execution fails. Failures include runtime errors (such as type errors and limit breaches) as well as panics.

```yaml
# Policy 1: Ignore the error and pass the request through (default)
errorPolicy: ignore

# Policy 2: Return an HTTP 500 error
errorPolicy: deny

# Policy 3: Return a custom status code and response body
errorPolicy:
  denyWith:
    status: 403
    body: "Script execution failed"
```

**ignore (default)**

When a script fails, `ignore` records its fixed error classification and continues to the next plugin. It does not roll back header or context changes made before the failure. This fail-open policy is suitable for non-critical enhancement logic; it does not soften `BodyAccess` failures.

Explicit `deny()`, `terminate()`, and `grpc_error()` results retain their normal semantics regardless of `errorPolicy`.

Request-body storage, scan-budget, range, mutation, and opaque-Body upload/read failures are classified internally as `BodyAccess`. They bypass `errorPolicy` and return the fixed HTTP 500 denial with client body `Request body processing failed` (gRPC delivery follows the normal terminal-response mapping). Only `request body access error` is recorded for this VM failure, without operational detail. This is a fixed safety boundary, not another configurable policy.

**deny**

When a script fails, the gateway returns HTTP 500 and aborts request processing. This is a fail-close policy, appropriate for security-critical scenarios such as authorization checks and access control, ensuring that a script failure never results in a request being mistakenly passed through.

**denyWith**

Similar to `deny`, but allows customizing the returned HTTP status code and response body. `status` must be in the range 200–599; `body` is a plain-text string. Use this when the client needs to receive a specific error format.

In production, use `deny` or `denyWith` for security-related scripts (permission checks, rate-limit marking, etc.) and keep `ignore` for enhancement scripts (adding debug headers, setting context values).

## Runtime Error Types

The following error types are raised by the VM at runtime. They are handled
according to `errorPolicy` except for the fixed hard-failure `BodyAccess`
boundary described below.

| Error Type | Trigger Condition | Description |
|------------|------------------|-------------|
| `TypeError` | Type mismatch in an operation (e.g., `"abc" - 1`) | Operand types do not satisfy operator requirements |
| `DivisionByZero` | Divisor is zero | Integer division by zero |
| `IntegerOverflow` | Arithmetic overflow | Result exceeds the i64 range |
| `StepLimitExceeded` | Execution step budget exhausted | Possible infinite loop or excessive computation |
| `LoopLimitExceeded` | Loop iteration limit exceeded | A single loop has too many iterations |
| `CallLimitExceeded` | Function call limit exceeded | Too many built-in function calls |
| `StackOverflow` | Stack depth limit exceeded | Expression nesting is too deep |
| `StringTooLong` | String length limit exceeded | Concatenation result exceeds `maxStringLen` |
| `RegexError` | Regex failure | Regex parsing or execution failed |
| `ApiError` | API call or argument validation failed | Includes PluginSession failures and invalid URL/method/headers or a direct IP literal rejected by the `http.call()` SSRF precheck; a DNS-resolved address rejected at dial time returns `status = 0` |
| `BodyAccess` | Request-body storage, scan, range, mutation, or opaque-Body upload/read failure | Fixed generic fail-close response; bypasses `errorPolicy` |

`TypeError` is the most common runtime error, typically arising when a script operates on a value that may be `nil` — for example, performing string operations on a request header that does not exist. It is recommended to perform `nil` checks on values read from external sources (such as `req.header()` and `ctx.get()`) within the script.

`ApiError` includes host-interface failures and invalid arguments; it need not be transient. It follows `errorPolicy`, retaining only the fixed `api error` classification in the VM failure log.

Outbound `http.call()` transport or response-stream failures, response-body overflow, and invalid bounded response conversion return the atomic failure map `{status: 0, body: "", headers: {}}`. They return no partial response and do not invoke `errorPolicy`. The response body cap is `min(httpMaxResponseBodyBytes, maxStringLen)`. This differs from an opaque request `Body` read or upload-accounting failure, which raises `BodyAccess` and always fails closed. See [http.call](./api-reference.md#httpcall) for the result contract.

## Panic Isolation

`execute_safe` uses an asynchronous `catch_unwind` boundary around the complete VM future in both execution phases. A captured unwinding panic does not escape into the caller. The panic payload is discarded; the only panic diagnostic it records is `vm panic` in `PluginLog`, followed by the configured `errorPolicy`. It does not attach the panic message, script name, or trigger details.

This boundary is a fallback, not a replacement for returning typed errors from fallible built-ins. It catches unwinding panics, not process aborts.

## Regex Security

The DSL's regex functionality is designed to defend against ReDoS (Regular Expression Denial of Service) attacks:

**Compile-time pre-compilation**: Regex patterns used with the `.matches()` method are compiled into a forward dense DFA during script compilation. The DFA is capped at 1 MiB and consumes the body incrementally, so matching does not require materializing the complete request body.

**Syntax validation**: Invalid regex patterns cause a compile-time error, rather than failing at runtime. This surfaces regex errors at script load time rather than exposing them under production traffic.

**Length and automaton limits**: Regex patterns are capped at 1,024 bytes and the compiled DFA at 1 MiB. The engine does not use backtracking; body input consumption is additionally bounded by `maxBodyScanPasses`. Unicode-aware word boundaries are not supported by the streaming matcher; use an explicit ASCII boundary such as `(?-u:\b)` when appropriate.

The pattern arguments to `regex_find()` and `regex_replace()` are subject to the same 1,024-byte limit.

## Execution Phase Restrictions

The DSL plugin can run in two phases. Available APIs differ between phases.

| Phase | Available APIs | Unavailable APIs | Notes |
|-------|---------------|-----------------|-------|
| `requestPlugins` | `req.*`, `ctx.*`, `deny`, `next`, `terminate`, `grpc_error` | `resp.set_status` (returns `nil`, has no effect) | Runs before the request is forwarded |
| `upstreamResponseFilterPlugins` | `req.*` (read-only), `resp.*`, `ctx.*`, `deny`, `next`, `grpc_error`, `terminate` | — | Runs during the response header phase |

In the response phase, `req.*` functions are still callable, but request-body reads can use only a body that was captured during the request phase. A late response-stage read never starts downstream capture. Request-body mutation methods are rejected during configuration validation outside `requestPlugins`; other request mutations such as `req.set_header()` cannot change a request that has already been forwarded.

In `requestPlugins`, `grpc_error()` sends a direct trailers-only gRPC response through the canonical terminal writer. In `upstreamResponseFilterPlugins`, it aborts the upstream response and carries the exact gRPC code and optional message to that same writer; the upstream response is not forwarded. In both phases, the wire HTTP status is `200`, while the gRPC application status is recorded separately in the access log as `grpc_status`. The code is not reconstructed from an HTTP status.

## Per-Function Limit Summary

Some built-in functions have their own parameter or result limits in addition to the global resource limits:

| Function | Limit | Description |
|----------|-------|-------------|
| `req.body.json_get` | Definite JSONPath up to 1,024 bytes and 32 segments; selected result bounded by `maxStringLen` | Selectively parses one value without building a complete JSON DOM |
| `req.body.json_select` | 1–64 static named paths; names up to 256 bytes; per-value and live aggregate string payload bounded by `maxStringLen` | Selects several values with one complete parse and Body scan |
| `req.body.read` | Non-negative range; requested raw length and output each bounded by `maxStringLen` | Reads the memory-backed or spilled body transparently |
| `req.body.matches` / other body scans | Total scanned bytes bounded by `maxBodyScanPasses × physical body cap` | Prevents repeated full-body scans |
| Request-body mutation methods | Result bounded by the physical body cap and 4,096 mutation segments | Mutations are transactional and request-stage only |
| `time_format` | Format string up to 256 characters | Prevents excessive resource consumption from overly long format strings |
| `regex_find` / `regex_replace` | Pattern up to 1,024 bytes | Guards against ReDoS |
| `req.header_names` / `resp.header_names` | Returns at most 256 header names | Guards against memory issues from requests with many headers |
| `resp.set_status` | Final status code must be in range 200–599 | Returns `ApiError` if out of range |
| `range` | Generates at most 1,000 elements | Guards against large lists consuming excessive memory |
| `http.call` | Timeout capped at `httpMaxTimeoutMs` (default 5,000 ms); opaque `Body` capped by `min(httpMaxRequestBodyBytes, physical body cap)` (default 1 MiB, configured maximum 32 MiB); response capped at `httpMaxResponseBodyBytes` (default 1 MiB); private IPs blocked when configured | Prevents gateway resource exhaustion and SSRF attacks |

When `req.body` is the third argument to `http.call`, the runtime pull-streams raw bytes
from an immutable call-time snapshot without full materialization. Fetched bytes consume
the cumulative body-scan budget. The caller cannot set `Content-Length` or
`Transfer-Encoding`; an empty `Body` sends `Content-Length: 0`, while `nil` attaches no
body. Opaque-Body calls return 3xx responses without following redirects. By contrast,
`Str` and `Nil` calls retain the existing redirect behavior.

`req.body.json_get()` accepts only a definite path rooted at `$`: `.member`, `["quoted member"]`, and non-negative `[index]` segments. Wildcards, recursive descent, filters, slices, unions, and dynamic paths are rejected at compile time.

`req.body.json_select()` accepts a non-empty literal Map of unique output names
to the same definite paths. It builds the shared-prefix plan during VM
initialization, not per request. Use it instead of repeated `json_get` calls
when several fields are needed.

`req.header_names()` and `resp.header_names()` return a list of strings. When the actual number of headers exceeds 256, only the first 256 are returned without raising an error. If you need to access all headers, use `req.header()` directly with specific header names rather than iterating over the full list.

## Related Documentation

- [API Reference](./api-reference.md) — Detailed signatures and usage for all built-in functions
- [Language Syntax Reference](./language-syntax.md) — Type system, operators, and control flow
- [Practical Examples](./examples.md) — Real-world script examples including security-related use cases
- [Bytecode Guide](./bytecode.md) — Using pre-compiled bytecode to eliminate compile-time overhead
