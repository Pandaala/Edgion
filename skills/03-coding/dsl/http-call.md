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

Successful calls use the HTTP status. Network, timeout, and response-stream failures
return status `0`, an empty body, and an empty header map. Response bodies are
streaming-capped, then UTF-8-safely truncated to
`min(httpMaxResponseBodyBytes, max_string_len)`.

The third argument accepts `Str`, `Nil`, or the opaque `Body` value obtained from
`req.body`. A `Body` call takes an immutable logical snapshot at call start and pulls
raw bytes from memory, spill files, and mutation segments without first materializing
the complete body as `String` or `Bytes`. The snapshot length supplies the exact
`Content-Length`; an empty `Body` therefore sends `Content-Length: 0`, while `Nil`
attaches no request body.

## Errors That Do Not Return Status 0

`call_builtin_http` raises `RuntimeError::ApiError` for invalid arity, malformed URL,
unsupported scheme or method, a blocked IP-literal host, or
caller-supplied `Content-Length` / `Transfer-Encoding` on an opaque `Body` call.
Invalid timeout, body, or header-map value types raise `RuntimeError::TypeError`.
Those errors flow through `DslErrorPolicy` (`Ignore`, `Deny`, or `DenyWith`); they
are not result maps.

Opaque-Body availability, read, scan-budget, and request-size-limit failures raise the
hard fail-close `RuntimeError::BodyAccess`, bypassing `DslErrorPolicy`. Network,
timeout, response-body stream, and dial-time resolved-address rejection failures use
the status-0 map. Preserve this split when changing validation or adding arguments.

## Runtime Limits

`DslRuntime` receives these fields from `DslConfig`:

- `httpMaxTimeoutMs` (default 5000)
- `httpMaxRequestBodyBytes` (default 1 MiB, explicit maximum 32 MiB)
- `httpMaxResponseBodyBytes` (default 1 MiB)
- `httpBlockPrivateIps` (default false)

Call timeouts are clamped to the configured maximum. For an opaque `Body`, the effective
request limit is the smaller of `httpMaxRequestBodyBytes` and the gateway's resolved
physical request-body cap. Bytes actually fetched from the snapshot count against the
VM's cumulative body-scan budget; unfetched bytes are refunded if the peer responds
early or stops reading.

The shared `Value` representation is `Value::Map(Box<IndexMap<String, Value>>)`; its
display/string conversion is `[map:<len>]`, and boxing preserves the `Value` size
invariant.

## SSRF Boundary

All process-global HTTP clients disable environment proxy inheritance. `Str` and `Nil`
calls follow at most 10 redirects. With private-IP blocking on, direct IP literals are
checked before sending, while every hostname is resolved through `EgressResolver`; the
resolver classifies the exact addresses handed to reqwest for dialing. Loopback, private,
link-local, unspecified, IPv4-mapped IPv6, ULA, and IPv6 link-local destinations are
blocked. Redirect hops use the same literal and dial-time DNS checks.

Opaque `Body` calls never follow redirects because their pull stream is intentionally
single-use; the 3xx response is returned to the script. This also avoids silently
changing methods or replaying request bytes.

The custom resolver closes the DNS-rebinding gap because reqwest dials the classified
addresses rather than independently resolving the hostname again. Network-level egress
controls remain recommended as defense in depth. Rejections of request-derived DSL
destinations increment the canonical egress metric without emitting a WARN per address,
which avoids client-driven log amplification.

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
  httpMaxRequestBodyBytes: 1048576
  httpMaxResponseBodyBytes: 65536
  httpBlockPrivateIps: true
  errorPolicy: deny
```

Integration coverage lives in
`edgion-tests/integration/conf/EdgionPlugins/Dsl/24_EdgionPlugins_dsl-http-call.yaml`
and the DSL client suite. Use `resp.set_header()` when a test client must observe a
computed value; `req.set_header()` changes only the upstream request.
