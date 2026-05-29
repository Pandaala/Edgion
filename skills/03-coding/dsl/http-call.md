# DSL http.call — Implementation Guide

## Overview

`http.call(method, url, body [, timeout_ms [, headers]])` is an async builtin in EdgionDSL that lets scripts make outbound HTTP requests. It never raises a runtime error — all failures (timeout, SSRF block, bad scheme, network error) return `status = 0`.

## Key files

| File | Role |
|------|------|
| `src/core/gateway/plugins/http/dsl/lang/builtins.rs` | `call_builtin_http` + SSRF helpers |
| `src/core/gateway/plugins/http/dsl/runtime.rs` | `DslRuntime` struct + global `DSL_HTTP_CLIENT` |
| `src/core/gateway/plugins/http/dsl/config.rs` | Config fields: `http_max_timeout_ms`, `http_max_response_body_bytes`, `http_block_private_ips` |
| `src/core/gateway/plugins/http/dsl/lang/bytecode.rs` | `OpCode::MakeMap(u16)`, `OpCode::IndexGet` |
| `src/core/gateway/plugins/http/dsl/lang/vm.rs` | `MakeMap` handler, `IndexGet` handler (Map + List), async `execute` / `execute_safe` |
| `src/core/gateway/plugins/http/dsl/lang/compiler/expr.rs` | `ExprKind::Index` → `OpCode::IndexGet` |
| `src/core/gateway/plugins/http/dsl/tests/http_call_tests.rs` | Unit + integration tests |

## Return value shape

```
Map {
  "status":  Int   // HTTP status code, or 0 on failure
  "body":    Str   // Response body (truncated at min(httpMaxResponseBodyBytes, max_string_len))
  "headers": Map   // Lowercase header name → last value for that name
}
```

The body is first streaming-capped at `max_response_body_bytes`, then silently truncated to
`max_string_len` (UTF-8 boundary aware) before being wrapped in `Value::Str`, so the effective
cap is `min(httpMaxResponseBodyBytes, max_string_len)` (default `min(1 MiB, 8 KiB)` = 8 KiB).
Why truncation rather than raising `StringTooLong` →
[http-call body cap](../../04-review/dsl-builtins/http-call-body-truncate-to-max-string-len.md).

## DslRuntime fields

```rust
pub struct DslRuntime {
    pub max_timeout_ms: u64,            // from httpMaxTimeoutMs (default 5000)
    pub max_response_body_bytes: usize, // from httpMaxResponseBodyBytes (default 1 MiB)
    pub block_private_ips: bool,        // from httpBlockPrivateIps (default false)
    // ... other fields
}
```

## Global HTTP client

`DSL_HTTP_CLIENT` is a `OnceLock<reqwest::Client>` initialized once with:
- `redirect::Policy::limited(10)`
- `pool_max_idle_per_host(4)`
- No per-plugin timeout or redirect override — limits are enforced per-call in `call_builtin_http`.

## SSRF protection

`is_ssrf_blocked_ip_literal(host: &str) -> bool` in `builtins.rs`:

1. Strips `%zone_id` suffix (IPv6 zone IDs).
2. Strips `[` `]` brackets — reqwest's `url` crate returns IPv6 hosts as `[::1]`.
3. Parses with `IpAddr::from_str`.
4. Checks `is_private_v4` (loopback + private + link-local + unspecified) or IPv4-mapped IPv6 (`to_ipv4_mapped()`) or IPv6 fc00::/7 + fe80::/10.

URL scheme validation runs before the IP check: only `http` and `https` are allowed; others return status 0 immediately.

## Async VM

`execute()` and `execute_safe()` in `vm.rs` are `async fn`. The bridge from sync Pingora upstream filter to async VM uses `tokio::task::block_in_place`.

`execute_safe` wraps with `AssertUnwindSafe` + `catch_unwind` to prevent panics from propagating.

## Value::Map

- Type: `Value::Map(IndexMap<String, Value>)` (preserves insertion order).
- `OpCode::MakeMap(n)`: pops `n` key-value pairs from the stack (reversed), builds map.
- `OpCode::IndexGet`: handles both `List` (Int key → string element) and `Map` (Str key → Value | Nil).
- `len()` builtin handles `Value::Map` → `Ok(Value::Int(m.len() as i64))`.
- `to_string` on `Map` → `"[map]"`.

## Config YAML example

```yaml
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "my-script"
        source: |
          let r = http.call("GET", "http://internal-svc/health", nil, 1000)
          if r["status"] != 200 {
            return deny(503, "backend down")
          }
        httpMaxTimeoutMs: 3000
        httpMaxResponseBodyBytes: 65536
        httpBlockPrivateIps: false
        errorPolicy: deny
```

## Adding new tests

Integration test config: `examples/test/conf/EdgionPlugins/Dsl/24_EdgionPlugins_dsl-http-call.yaml`
Test client: `examples/code/client/suites/edgion_plugins/dsl/dsl.rs` — see `test_http_call_success` (scenario 24).

Use `resp.set_header()` in DSL test scripts when you need to assert on the result from the test client — `req.set_header()` only reaches the backend echo, not the test client response.
