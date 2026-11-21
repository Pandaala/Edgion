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
| `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/http_call_tests.rs` | Lock-in tests |

Gateway `dsl/config.rs` is a re-export, not the schema owner.

## Return Shape

```text
Map {
  "status":  Int
  "body":    Str
  "headers": Map
}
```

Successful calls use the HTTP status. Network, timeout, dial-time egress, and
response-processing failures return status `0`, an empty body, and an empty header
map. The response body hard cap is
`min(httpMaxResponseBodyBytes, max_string_len)`: a declared or streamed overflow
fails the call, never returns a truncated prefix, and an exact-cap body succeeds only
after EOF. Invalid UTF-8 is converted lossily only after its exact expanded byte
length is proven to fit `max_string_len`.

Response header names are lowercased. Header count keeps the existing raw-traversal
prefix rule: before inspecting each raw header, processing stops if the retained Map
already has `max_map_size` names. Within the inspected raw prefix, non-UTF-8 values
are skipped and a duplicate replaces an earlier value only if it is reached before
the Map becomes full. Every retained name and value must fit `max_string_len`,
otherwise the complete call returns the status-zero failure map.

The current retained names plus values also have a fixed 1 MiB cumulative
payload cap, checked before each copy. Duplicate replacement refunds the old
value and counts its name once. Overflow returns the same failure Map before
body consumption. This producer cap is separate from the canonical client's
decoded HTTP/2 header-list limit (16 KiB default, 64 KiB maximum).

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
timeout, dial-time resolved-address rejection, response-body stream errors, response
overflow, lossy UTF-8 expansion overflow, and retained response-header string
overflow use the status-0 map. Preserve this split when changing validation or adding
arguments.

## Runtime Limits

`DslRuntime` receives these fields from `DslConfig`:

- `httpMaxTimeoutMs` (default 5000, range 1–60000 ms)
- `httpMaxRequestBodyBytes` (default 1 MiB, explicit maximum 32 MiB)
- `httpMaxResponseBodyBytes` (default and explicit maximum 1 MiB)
- `httpBlockPrivateIps` (default false). Built-in non-dialable addresses and configured Gateway blocked CIDRs apply independently to literals, DNS answers and redirect destinations.

Call timeouts are clamped to the configured maximum. For an opaque `Body`, the effective
request limit is the smaller of `httpMaxRequestBodyBytes` and the gateway's resolved
physical request-body cap. Bytes actually fetched from the snapshot count against the
VM's cumulative body-scan budget; unfetched bytes are refunded if the peer responds
early or stops reading.

Response aggregation uses the canonical outbound streaming cap. It does not use
request `BodyStorage`, does not add response-side storage, and does not change the
main proxy upstream's always-streaming response path.

The shared `Value` representation is `Value::Map(Box<IndexMap<String, Value>>)`; its
display/string conversion is `[map:<len>]`, and boxing preserves the `Value` size
invariant.

## SSRF Boundary

All process-global HTTP clients disable environment proxy inheritance. `Str` and `Nil`
calls follow at most 10 redirects. Both modes check direct IP literals before sending
and filter the exact DNS addresses handed to reqwest. Only strict private blocking
or a configured Gateway CIDR list selects the DSL answer cache. Otherwise the uncached
system lookup filters socket addresses and retains their IPv6 interface scope. Enabling private-IP blocking additionally rejects loopback, private
(including shared IPv4 space), unique-local, link-local and unspecified destinations,
including their IPv4-mapped forms. Redirect hops apply the same checks.

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

## Request-body aggregate capacity

Body-source capacity failures propagate as `RuntimeError::BodyCapacityExceeded`
under the [canonical builtin error contract](builtin-error-semantics.md#request-body-aggregate-capacity).
This does not change outbound-call `ApiError` behavior or outbound response limits.
