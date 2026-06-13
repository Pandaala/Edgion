# TraceContext Plugin

> **🔌 Edgion extension**
>
> TraceContext is a W3C Trace Context propagation plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## What does TraceContext solve?

When an OpenTelemetry-instrumented service mesh sits behind the gateway, each backend service needs to know which distributed trace a request belongs to. The W3C Trace Context specification ([traceparent](https://www.w3.org/TR/trace-context/) header) is the interoperable standard that links spans across service boundaries.

The TraceContext plugin makes the gateway a **propagation-only** participant in that standard:

- The gateway reads and forwards `traceparent` / `tracestate` headers, keeping inbound context intact.
- When no valid context exists, the gateway can generate a fresh root span (a new 128-bit trace ID).
- The gateway does **not** export spans to a collector — it is not a tracing backend. The aligned `x-trace-id` surfaced in access logs and response headers is the 32-hex trace ID taken from `traceparent`.

This is opt-in: attaching no `TraceContext` plugin to a route means the gateway passes headers through without interpreting them.

## Quick Start

### Simplest configuration (transparent pass-through with auto-generation)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: trace-context-default
  namespace: edgion-default
spec:
  requestPlugins:
    - type: TraceContext
      config:
        generateWhenMissing: true
        trustInbound: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: trace-context-route
  namespace: edgion-default
spec:
  parentRefs:
    - name: edgion-plugins-gateway
      namespace: edgion-test
  hostnames:
    - "trace-context.example.com"
  rules:
    - matches:
        - path: { type: PathPrefix, value: "/" }
      filters:
        - type: ExtensionRef
          extensionRef: { group: edgion.io, kind: EdgionPlugins, name: trace-context-default }
      backendRefs:
        - name: test-http
          namespace: edgion-test
          port: 30001
```

### Default value summary

| Field | Default | Meaning |
|-------|---------|---------|
| `generateWhenMissing` | `true` | Generate a fresh root `traceparent` when the inbound request has none |
| `echoToClient` | `false` | Do not copy `traceparent` onto the client response |
| `trustInbound` | `true` | Honor a well-formed inbound `traceparent` (transparent pass-through) |
| `defaultSampled` | `false` | When generating a root span, set trace-flags to `00` (defer sampling to downstream) |

---

## Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `generateWhenMissing` | Boolean | `true` | When `true`, if the inbound request carries no valid `traceparent` (absent or malformed), the gateway generates a fresh 128-bit trace ID and injects a valid root `traceparent` upstream. Any partial or garbage trace headers are stripped before forwarding. When `false`, the gateway takes no action when trace context is absent. |
| `echoToClient` | Boolean | `false` | When `true`, write the effective `traceparent` (forwarded or newly generated) back onto the HTTP response sent to the client. Useful for single-page applications or mobile clients that need to correlate their UI activity to a backend trace. |
| `trustInbound` | Boolean | `true` | When `true`, a well-formed inbound `traceparent` is forwarded unchanged to the upstream (transparent pass-through); the gateway aligns its `x-trace-id` to the W3C 32-hex trace_id. When `false`, the gateway ignores inbound trace context entirely and always originates a fresh root — recommended for internet-edge deployments where clients cannot be trusted to supply valid trace context. |
| `defaultSampled` | Boolean | `false` | Controls the trace-flags byte in a gateway-generated `traceparent`. `false` → `00` (not sampled; lets the downstream collector decide). `true` → `01` (sampled; forces all downstream services to sample this trace). Has no effect when an inbound `traceparent` is forwarded unchanged. |

---

## Propagation behavior

### Case 1 — Well-formed inbound traceparent (`trustInbound: true`)

The gateway forwards the `traceparent` header unchanged to the upstream. `tracestate` is also forwarded unless it exceeds 512 bytes (in that case it is dropped). The gateway's `x-trace-id` in access logs and the optional response header is aligned to the 32-hex trace_id embedded in `traceparent`.

### Case 2 — Absent or malformed inbound traceparent

If `generateWhenMissing: true` (default), the gateway generates a fresh root `traceparent` with a new 128-bit trace ID and injects it upstream. The request becomes the trace root for all downstream services. Any partial or garbage `traceparent` / `tracestate` headers from the client are stripped before forwarding.

If `generateWhenMissing: false`, the gateway makes no changes; no trace headers are injected.

### Case 3 — `trustInbound: false` (internet-edge posture)

The gateway ignores any inbound `traceparent` / `tracestate` entirely, regardless of whether they are well-formed. A fresh root `traceparent` is always generated (as if the request had arrived without any trace headers). This neutralizes client trace spoofing and prevents external identifiers from polluting your internal trace trees.

---

## Security note — when to set `trustInbound: false`

At an internet-facing edge, a client can supply an arbitrary `traceparent` and cause your internal traces to appear to descend from an external span ID. Consequences include:

- **Log pollution**: external trace IDs appear in your access logs and observability dashboards.
- **Trace spoofing**: a fabricated `traceparent` may make a malicious request appear to belong to a legitimate trace.

Set `trustInbound: false` on public-facing routes or wherever the caller population is untrusted. On internal service mesh routes (gateway behind a service mesh sidecar) `trustInbound: true` (the default) is safe and preserves end-to-end trace correlation.

---

## Plugin chain constraints

### At most one TraceContext per route

The controller rejects an `EdgionPlugins` resource that declares more than one `TraceContext` in the same request plugin chain. Configure exactly one instance per route.

### Ordering with RequestMirror

When the same plugin chain contains both `TraceContext` and [RequestMirror](./request-mirror.md), declare `TraceContext` **before** `RequestMirror`. The mirror is dispatched after the request-phase plugins run in order; placing `TraceContext` first ensures the mirrored request carries the same aligned `traceparent` as the primary request.

```yaml
spec:
  requestPlugins:
    # 1. Align / generate trace context first
    - type: TraceContext
      config:
        generateWhenMissing: true
    # 2. Mirror carries the aligned traceparent
    - type: RequestMirror
      config:
        mirrors:
          - backendRef:
              name: shadow-service
              port: 8080
```

---

## Scope and compatibility

Only the W3C Trace Context `traceparent` header is supported. The plugin does **not** handle:

- Zipkin B3 headers (`X-B3-TraceId`, `X-B3-SpanId`, `X-B3-Flags`, `b3`)
- Jaeger `uber-trace-id`
- AWS X-Ray `X-Amzn-Trace-Id`

However, any OpenTelemetry-instrumented downstream service — regardless of its exporter backend (Jaeger, Zipkin, Tempo, Datadog, etc.) — speaks W3C Trace Context natively, so this plugin provides full interoperability with all OTLP-compatible observability stacks.

---

## Complete example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: trace-context-edge
  namespace: production
spec:
  requestPlugins:
    - type: TraceContext
      config:
        generateWhenMissing: true
        trustInbound: false   # internet-facing: always originate a fresh root
        echoToClient: true    # surface traceparent on the response for client correlation
        defaultSampled: false # let the downstream collector decide sampling
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: trace-context-edge
      backendRefs:
        - name: api-service
          port: 8080
```

---

## FAQ

### Q1: My downstream Jaeger/Zipkin UI is not showing gateway-originated spans?

**A**: TraceContext is propagation-only — the gateway does not export spans. It injects a `traceparent` so that your downstream services can record spans that share the same trace ID. Configure your downstream service's OpenTelemetry SDK with a Jaeger/Zipkin exporter; the gateway's role is only to start and carry the trace ID.

### Q2: I set `echoToClient: true` but the browser does not see the header?

**A**: Check that your CORS policy (via the [CORS plugin](./cors.md)) exposes `traceparent` in `Access-Control-Expose-Headers`. Browsers suppress response headers that are not in the expose list.

### Q3: The controller rejected my EdgionPlugins resource?

**A**: The most common reason is declaring more than one `TraceContext` in the same `requestPlugins` list. Remove the duplicate.

### Q4: Should I use `defaultSampled: true`?

**A**: Only when you want the gateway to force 100 % sampling for all traces it originates. This overrides any downstream sampling decision and can increase cost significantly in high-traffic environments. Prefer `false` (the default) and configure sampling rates in your collector.

---

## Related links

- [Request Mirror Plugin](./request-mirror.md)
- [CORS Plugin](./cors.md)
- [Filters overview](../README.md)
