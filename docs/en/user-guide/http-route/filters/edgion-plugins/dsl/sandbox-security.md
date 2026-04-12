# Sandbox Security Model

This document describes the security mechanisms of the EdgionDSL script execution environment, including resource limits, error handling policies, and API availability per execution phase.

## Security Design Philosophy

The EdgionDSL sandbox is built on the following core principles:

**Zero-capability by default**: Scripts start with no external interaction capabilities whatsoever. There is no filesystem access, no network calls, no environment variable reads, and no process management. All interactions with the gateway context (reading request headers, rejecting requests, etc.) must go through audited built-in functions whose behavior is well-defined and bounded.

**Defense in depth**: Security is enforced at two layers. Compile-time validation statically checks structural constraints — instruction count, variable count, constant pool size, and so on — when a script is loaded, rejecting scripts that are overly complex or maliciously crafted. Runtime limits dynamically track execution steps, loop iterations, and function call counts on every request, preventing scripts from exceeding expected resource consumption at runtime. The two layers are complementary; a failure in one does not compromise the other.

**Panic isolation**: The VM's execution function wraps the entire run in `std::panic::catch_unwind`. Even if an unexpected panic occurs inside the VM, it will not propagate to the gateway's main request-handling process. The gateway gracefully degrades according to the configured `errorPolicy` and records the panic details for post-mortem analysis.

**Observability**: Every execution error, resource limit breach, or panic produces a structured log entry containing the script name, error type, and triggering cause, making it straightforward to diagnose issues in production.

## Runtime Resource Limits

The following limits are tracked independently on each request and do not interfere with one another. All configuration fields are located inside the plugin's `config` block.

| Limit | Config Field | Default | Min | Max | Description |
|-------|-------------|---------|-----|-----|-------------|
| Max execution steps | `maxSteps` | 10,000 | 1 | 1,000,000 | Total bytecode instructions per execution |
| Max loop iterations | `maxLoopIterations` | 100 | 1 | 100,000 | Maximum iterations for a single loop body |
| Max function calls | `maxCallCount` | 500 | 1 | 100,000 | Total number of built-in function calls |
| Max stack depth | `maxStackDepth` | 128 | 1 | 1,024 | Value stack depth |
| Max string length | `maxStringLen` | 8,192 | 1 | 1,048,576 | Maximum bytes in a string concatenation result |

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

### Configuration Example

```yaml
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "my-script"
        source: |
          # script content
        maxSteps: 5000
        maxLoopIterations: 50
        maxCallCount: 200
        maxStackDepth: 64
        maxStringLen: 4096
        errorPolicy: deny
```

Using the default values is recommended for production. Only raise a specific limit when a script genuinely needs to handle larger data volumes or more complex logic — do not blindly increase all limits at once.

## Compile-Time Validation

When a script is loaded — compiled on first request in `source` mode, or validated at gateway startup in `bytecode` mode — compile-time validation is performed. The following limits are checked statically at compile time; a script that fails any of them is rejected and will not be loaded.

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

Regex patterns are not only checked for length at compile time — their syntax is also validated. An invalid regex causes a compile-time error when the script is loaded, rather than failing silently at runtime. Patterns used with the `.matches()` method are pre-compiled and cached at compile time, eliminating per-request regex compilation overhead.

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

When a script fails, the error is silently discarded and the request continues to be forwarded normally, as if the script had not run. This is a fail-open policy, suitable for non-critical enhancement logic (such as injecting debug headers or recording context information). Script errors do not affect normal request processing under this policy, but they are still written to the error log.

Note: The `ignore` policy applies only to unexpected errors. If a script explicitly calls `deny()` or `terminate()`, those calls are part of the script's normal logic and are not affected by `errorPolicy` — they execute with their defined semantics regardless.

**deny**

When a script fails, the gateway returns HTTP 500 and aborts request processing. This is a fail-close policy, appropriate for security-critical scenarios such as authorization checks and access control, ensuring that a script failure never results in a request being mistakenly passed through.

**denyWith**

Similar to `deny`, but allows customizing the returned HTTP status code and response body. `status` must be in the range 100–599; `body` is a plain-text string. Use this when the client needs to receive a specific error format.

In production, use `deny` or `denyWith` for security-related scripts (permission checks, rate-limit marking, etc.) and keep `ignore` for enhancement scripts (adding debug headers, setting context values).

## Runtime Error Types

The following error types are raised by the VM at runtime. When triggered, they are handled according to `errorPolicy`.

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
| `ApiError` | API call failure | An exception occurred in the PluginSession interface |

`TypeError` is the most common runtime error, typically arising when a script operates on a value that may be `nil` — for example, performing string operations on a request header that does not exist. It is recommended to perform `nil` checks on values read from external sources (such as `req.header()` and `ctx.get()`) within the script.

`ApiError` occurs when a built-in function fails to interact with the gateway's internal interfaces. These errors are generally transient and will not affect the request under the `ignore` policy.

## Panic Isolation

The VM's `execute_safe` function wraps the entire execution in Rust's `std::panic::catch_unwind`. This means:

- Even if an unexpected internal state triggers a panic inside the VM, that panic will not propagate upward to the gateway's main request-handling flow.
- Once `catch_unwind` captures the panic, the VM converts it into an execution error, which is then handled according to the currently configured `errorPolicy`.
- The gateway logs the panic details — including the panic message and script name — to the error log for diagnostic purposes.

Panic isolation is the last line of defense, providing protection when neither normal script logic nor standard runtime error handling covers an edge case. For well-tested scripts under normal load, this mechanism should never be triggered.

## Regex Security

The DSL's regex functionality is designed to defend against ReDoS (Regular Expression Denial of Service) attacks:

**Compile-time pre-compilation**: Regex patterns used with the `.matches()` method are parsed and compiled during script compilation, and the compiled result is cached. Subsequent requests use the cached compiled regex directly, avoiding per-request regex compilation and eliminating compilation-phase latency.

**Syntax validation**: Invalid regex patterns cause a compile-time error, rather than failing at runtime. This surfaces regex errors at script load time rather than exposing them under production traffic.

**Length limit**: Regex patterns are capped at 1,024 bytes, preventing excessively long patterns from consuming too much memory. Extremely complex patterns (such as those with many backreferences or deeply nested groups) could theoretically still cause long match times; the `maxSteps` limit serves as an additional safeguard in those cases.

The pattern arguments to `regex_find()` and `regex_replace()` are subject to the same 1,024-byte limit.

## Execution Phase Restrictions

The DSL plugin can run in two phases. Available APIs differ between phases.

| Phase | Available APIs | Unavailable APIs | Notes |
|-------|---------------|-----------------|-------|
| `requestPlugins` | `req.*`, `ctx.*`, `deny`, `next`, `terminate`, `grpc_error` | `resp.set_status` (returns `nil`, has no effect) | Runs before the request is forwarded |
| `upstreamResponseFilterPlugins` | `req.*` (read-only), `resp.*`, `ctx.*`, `deny`, `next` | `grpc_error`, `terminate` | Runs during the response header phase |

In the response phase, `req.*` functions are still callable, but mutation operations such as `req.set_header()` have no effect on a request that has already been forwarded — they are useful only for reading request information.

`grpc_error()` is only effective in the request phase, because gRPC status codes are conveyed via HTTP/2 trailers, and trailers can only be set before the response body is sent. Calling `grpc_error()` in the response phase degrades to `terminate()` behavior and produces a warning log entry.

## Per-Function Limit Summary

Some built-in functions have their own parameter or result limits in addition to the global resource limits:

| Function | Limit | Description |
|----------|-------|-------------|
| `json_get` | Path up to 32 segments; JSON size bounded by `maxStringLen` | Guards against deeply nested path access |
| `time_format` | Format string up to 256 characters | Prevents excessive resource consumption from overly long format strings |
| `regex_find` / `regex_replace` | Pattern up to 1,024 bytes | Guards against ReDoS |
| `req.header_names` / `resp.header_names` | Returns at most 256 header names | Guards against memory issues from requests with many headers |
| `resp.set_status` | Status code must be in range 100–599 | Returns `ApiError` if out of range |
| `range` | Generates at most 1,000 elements | Guards against large lists consuming excessive memory |

The path segment limit for `json_get` (32 segments) refers to the number of levels in the path expression — for example, `"a.b.c.d"` has 4 segments. Paths deeper than 32 levels are extremely rare in real-world use cases, and this limit will not impact normal usage.

`req.header_names()` and `resp.header_names()` return a list of strings. When the actual number of headers exceeds 256, only the first 256 are returned without raising an error. If you need to access all headers, use `req.header()` directly with specific header names rather than iterating over the full list.

## Related Documentation

- [API Reference](./api-reference.md) — Detailed signatures and usage for all built-in functions
- [Language Syntax Reference](./language-syntax.md) — Type system, operators, and control flow
- [Practical Examples](./examples.md) — Real-world script examples including security-related use cases
- [Bytecode Guide](./bytecode.md) — Using pre-compiled bytecode to eliminate compile-time overhead
