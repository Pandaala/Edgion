# DSL `http.call` Implementation Guide

`http.call(method, url, body [, timeout_ms [, headers]])` is an asynchronous outbound
HTTP builtin. Its failures are intentionally split between runtime errors and a result
map; scripts using a deny error policy must understand that distinction.

## Ownership and Key Files

| File | Responsibility |
|------|----------------|
| `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs` | `DslConfig` and HTTP limit/security fields |
| `edgion-dsl/src/bytecode.rs`, `compiler/` | Builtin ID, bytecode, static compilation |
| `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` | Argument validation, SSRF checks, request/result mapping |
| `edgion-gateway/src/plugins/http/dsl/runtime.rs` | `DslRuntime` and the permissive/guarded reqwest clients |
| `edgion-gateway/src/plugins/http/dsl/lang/vm.rs` | Async execution and panic isolation |
| `edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs` | Lock-in tests |

Gateway `dsl/config.rs` is a re-export, not the schema owner.

## Return Shape

```text
Map {
  "status":  Int
  "body":    Str
  "headers": Map
}
```

Successful calls use the HTTP status. Request/send/body-read failures return status `0`,
an empty body, and an empty header map. Response bodies are streaming-capped, then
UTF-8-safely truncated to `min(httpMaxResponseBodyBytes, max_string_len)`.

## Errors That Do Not Return Status 0

`call_builtin_http` raises `RuntimeError::ApiError` for invalid arity, malformed URL,
unsupported scheme or method, and a private/literal address blocked by policy. Invalid
timeout or header-map value types raise `RuntimeError::TypeError`. Those errors flow
through `DslErrorPolicy` (`Ignore`, `Deny`, or `DenyWith`); they are not result maps.

Only transport/request and response-body stream failures use the status-0 map. Preserve
this split when changing validation or adding arguments.

## Runtime Limits

`DslRuntime` receives these fields from `DslConfig`:

- `httpMaxTimeoutMs` (default 5000)
- `httpMaxResponseBodyBytes` (default 1 MiB)
- `httpBlockPrivateIps` (default false)

Call timeouts are clamped to the configured maximum. The shared `Value` representation is
`Value::Map(Box<IndexMap<String, Value>>)`; its display/string conversion is
`[map:<len>]`, and boxing preserves the `Value` size invariant.

## SSRF Boundary

Both process-global HTTP clients disable environment proxy inheritance. With private-IP
blocking off, the permissive client follows at most 10 redirects. With blocking on, the
guarded client checks the initial literal address and every redirect hop; loopback,
private, link-local, unspecified, IPv4-mapped IPv6, ULA, and IPv6 link-local literals are
blocked.

This is not complete hostname protection: a hostname that resolves to a private address
is not rejected by the literal check, and DNS rebinding remains possible. Production
deployments must enforce egress policy/firewall controls in addition to
`httpBlockPrivateIps`.

## Example

```yaml
type: Dsl
config:
  source: |
    let r = http.call("GET", "https://status.example.com/health", nil, 1000)
    if r["status"] != 200 {
      return deny(503, "backend down")
    }
  httpMaxTimeoutMs: 3000
  httpMaxResponseBodyBytes: 65536
  httpBlockPrivateIps: true
  errorPolicy: deny
```

Integration coverage lives in
`edgion-tests/integration/conf/EdgionPlugins/Dsl/24_EdgionPlugins_dsl-http-call.yaml`
and the DSL client suite. Use `resp.set_header()` when a test client must observe a
computed value; `req.set_header()` changes only the upstream request.
