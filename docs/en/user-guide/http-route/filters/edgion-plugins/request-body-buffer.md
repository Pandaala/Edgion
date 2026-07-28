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
skipped—the late read of a request that declared a body fails according to that
consumer's error policy. A request that truthfully declared no body still reads
as the canonical empty body and needs no cache. Capture refusals and
physical-cap failures remain fail-close.

Wasm modules needing the complete request body should normally run in
`requestPlugins` and export `proxy_on_request_body`; response-stage Wasm host
phases do not expose this cached request body.
