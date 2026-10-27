# RequestBodyBuffer Plugin

`RequestBodyBuffer` explicitly captures the proxied request body during
`requestPlugins` and keeps it in the request-scoped shared body cache. A later
plugin, such as a DSL entry in `upstreamResponseFilterPlugins`, can then read
the cached request body after it has been forwarded upstream.

```yaml
spec:
  requestPlugins:
    - type: RequestBodyBuffer
      config: {}
      body:
        maxBodySize: 1MiB
  upstreamResponseFilterPlugins:
    - type: Dsl
      config:
        name: inspect-request-after-upstream
        source: |
          if req.body == "expected" {
            resp.set_header("X-Body-Matched", "true")
          }
        errorPolicy: deny
```

The plugin is valid only in `requestPlugins`. It has no config fields; the
entry-level `body` block optionally adds conditions and a logical size limit.
Capture uses the gateway's single shared `BodyStorage`, including its
memory-to-disk spill behavior and physical body cap.

`EdgionGatewayConfig.spec.requestBody` controls whether capture is available,
the default memory spill threshold, and the physical body cap. Its optional
maximum fields override the corresponding defaults. Disabling capture makes
this plugin fail closed with the normal body-unavailable response; bodyless
requests still use the canonical empty body.

A late consumer never starts body capture. If no earlier plugin populated the
cache—for example because `RequestBodyBuffer` was absent or its body conditions
skipped—a late read of a request that declared a body fails closed with HTTP
500 and the fixed body `Request body processing failed`, regardless of the
consumer's `errorPolicy`. A request that truthfully declared no body still
reads as the canonical empty body and needs no cache.

Capture failures happen at the pre-VM body gate and have an empty response
body: request-body limit failures return 413, timeouts return 408, process-wide
storage capacity exhaustion returns 503, and refusals or other failures return 500. These gate responses are distinct from
the fixed generic 500 used when a later VM Body operation cannot access or
process an already-required cache. Aggregate capacity exhaustion remains a fixed
503 for those later VM operations as well.

DSL sees the cache as an opaque `req.body` value. Methods such as
`byte_len()`, `contains()`, `matches()`, `sha256()`, and `json_get()` inspect
it without requiring a complete string copy; `read(offset, max_bytes)` creates a
bounded string only for the requested result. All methods behave the same for
memory-backed and spilled storage.

Wasm modules needing the complete request body should normally run in
`requestPlugins` and export `proxy_on_request_body`; response-stage Wasm host
phases do not expose this cached request body.

Capture and retained payloads share the startup-only
[`gateway.request_body_budget`](../../../../ops-guide/configuration/gateway-config.md#process-wide-request-body-storage-admission)
limits across listeners. Capacity failure is always fail-closed, including DSL and
Wasm consumers configured to ignore ordinary errors. It is distinct from a 413
single-request limit and from a 500 storage I/O failure.
