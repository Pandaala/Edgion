# ExtProc Plugin

> **🔌 Edgion extension**
>
> ExtProc hands request/response phases to an external processing service over a gRPC bidirectional stream. It is provided by the `EdgionPlugins` CRD and is **wire-compatible with Envoy `envoy.service.ext_proc.v3`**, so existing ext_proc servers and SDKs work unchanged. It is an independent plugin, a peer of [ForwardAuth](./forward-auth.md) — the two can be combined.

## What does ExtProc solve?

Sometimes header logic is too dynamic to express as static config: rewriting headers from an external policy engine, enriching requests from a side service, or letting a separate team own request/response decisions in their own language. ExtProc delegates those decisions to an external gRPC service:

- The gateway sends request/response phases to the processor as `ProcessingRequest` messages.
- The processor replies with `ProcessingResponse` messages that can **mutate headers**, replace a buffered request body, or return an **`ImmediateResponse`** that short-circuits the request (e.g. a `403`).
- Two "gRPC"s are involved — don't conflate them: the **control channel** (gateway ↔ processor) is always gRPC; the **proxied traffic** is ordinary HTTP. ExtProc wraps each HTTP phase in gRPC and feeds it to the processor.

### Scope (this release)

| Capability | Supported | Phase |
|---|:---:|---|
| Read / modify **request** headers | ✅ | `RequestFilter` |
| Request-phase short-circuit (`ImmediateResponse`, e.g. 403) | ✅ | `RequestFilter` |
| Read / modify **response** headers | ✅ | async `UpstreamResponse` |
| Response-phase short-circuit (`ImmediateResponse`) | ✅ | async `UpstreamResponse` |
| Read / replace **request body** | ✅ | `RequestFilter`, `requestBodyMode: BUFFERED` |
| Read / modify **response body** | ❌ | Proxied upstream responses remain streaming-only |

`STREAMED` and `BUFFERED_PARTIAL` request modes are rejected. Response body
modes other than `NONE`/`SKIP` are also rejected; they are never silently
ignored.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ext-proc-basic
  namespace: edgion-default
spec:
  # Request-header processing
  requestPlugins:
    - type: ExtProc
      config:
        grpcService:
          target: "ext-proc.svc.cluster.local:9000"
          timeoutMs: 200
        processingMode:
          requestHeaderMode: SEND
          requestBodyMode: BUFFERED
        failureModeAllow: false   # fail-close if the processor is unreachable
  # Response-header processing (separate entry, async stage)
  upstreamResponsePlugins:
    - type: ExtProc
      config:
        grpcService:
          target: "ext-proc.svc.cluster.local:9000"
          timeoutMs: 200
        processingMode:
          responseHeaderMode: SEND
        failureModeAllow: false
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: ext-proc-route
  namespace: edgion-default
spec:
  parentRefs:
    - name: edgion-plugins-gateway
      namespace: edgion-test
  hostnames:
    - "ext-proc.example.com"
  rules:
    - matches:
        - path: { type: PathPrefix, value: "/" }
      filters:
        - type: ExtensionRef
          extensionRef: { group: edgion.io, kind: EdgionPlugins, name: ext-proc-basic }
      backendRefs:
        - name: test-http
          namespace: edgion-test
          port: 30001
```

> The request phase and the response phase are **separate entries** (`requestPlugins` vs `upstreamResponsePlugins`). List only the phase(s) you need. Each entry only reads its own phase's mode (`requestHeaderMode` for a `requestPlugins` entry, `responseHeaderMode` for an `upstreamResponsePlugins` entry).

## Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `grpcService.target` | String | — (required) | The processor's gRPC endpoint as a bare `host:port` authority (not a URL). IPv6 must be bracketed: `[::1]:9000`. |
| `grpcService.timeoutMs` | Integer | `200` | Per-message timeout (also bounds connect and the verdict wait). Must be `1..=60000`. |
| `grpcService.tls` | Boolean | `false` | Dial the target over TLS (server-auth, system/webpki roots). `false` = plaintext h2c (in-cluster default). |
| `processingMode.requestHeaderMode` | Enum | `SEND` | `SEND` to send request headers to the processor, `SKIP` to skip. Only read by a `requestPlugins` entry. |
| `processingMode.responseHeaderMode` | Enum | `SKIP` | `SEND` / `SKIP` for response headers. Only read by an `upstreamResponsePlugins` entry — set it to `SEND` there. |
| `processingMode.requestBodyMode` | Enum | `NONE` | `NONE`, `SKIP`, or `BUFFERED`. In `BUFFERED` mode, request headers (`end_of_stream=false`) and the full request body (`end_of_stream=true`) are sent on one `Process` stream. `STREAMED` and `BUFFERED_PARTIAL` are rejected. |
| `processingMode.responseBodyMode` | Enum | `NONE` | Only `NONE` or `SKIP`; proxied upstream responses remain streaming-only. |
| `failureModeAllow` | Boolean | `false` | When the processor is unreachable, times out, or returns an unusable verdict: `false` (fail-close) short-circuits the request with a `500`; `true` (fail-open) skips processing and continues. |

## Behavior

### Header mutation

The processor returns a `HeaderMutation` (`set_headers` + `remove_headers`). Each `set_headers` entry carries an `append_action`:

| `append_action` | Effect |
|---|---|
| `APPEND_IF_EXISTS_OR_ADD` (default) | Append a value (multi-value header). |
| `ADD_IF_ABSENT` | Set only if the header is currently absent. |
| `OVERWRITE_IF_EXISTS_OR_ADD` | Set (overwrite). |
| `OVERWRITE_IF_EXISTS` | Set only if the header is currently present. |

The gateway sends the phase's headers to the processor, including the read-only HTTP/2 pseudo-headers (`:method`, `:path`, `:authority` on the request; `:status` on the response) so the processor can route on them. Multi-valued headers (e.g. `Set-Cookie`) are preserved, not collapsed.

### Protected headers (security)

The processor **cannot** mutate:

- **Pseudo-headers** (`:method`, `:path`, `:status`, …) — preventing routing/method tampering.
- **Hop-by-hop / framing headers** — `Content-Length`, `Transfer-Encoding`, `Connection`, `Keep-Alive`, `Upgrade`, `TE`, `Trailer`, `Proxy-Connection`. These are owned by the proxy; allowing an external service to set them would enable request/response smuggling. (Mirrors Envoy's default `disallow_system_headers` posture.)

Any attempt to mutate a protected header is dropped and noted in the access log.

### Buffered request body

With `requestBodyMode: BUFFERED`, the request body is buffered before ExtProc
runs. The processor may return a `BodyMutation` in its `RequestBodyResponse`:
`body` replaces the full request body and `clear_body: true` replaces it with an
empty body. The gateway then removes `Transfer-Encoding` and sets the upstream
`Content-Length` to the final body size.

For a request that arrived without a body, the body phase still carries the
canonical empty byte sequence. A processor may replace it with a non-empty
body; that committed version is visible to later request plugins and is
replayed to the upstream with corrected framing.

Headers and body use one interactive `Process` stream. The gateway sends the
headers first and waits for their matching verdict before sending the original
body. A headers verdict with `CONTINUE_AND_REPLACE` applies its header mutation,
commits its body mutation (if present), and stops the request direction without
sending the original body. A `CONTINUE` headers verdict does not apply a
header-phase body mutation. In `BUFFERED` mode, a `RequestBodyResponse` may
apply both header and body mutations.

### ImmediateResponse (short-circuit)

If the processor returns an `ImmediateResponse`, the gateway stops proxying and returns that status + body to the client. In the **request** phase the response's custom header mutations are applied to the reply; in the **response** phase only the status and body are applied (the runtime rebuilds the reply, so response-phase `ImmediateResponse` custom headers are not preserved — and on gRPC routes the reply is status-only).

### Failure handling (`failureModeAllow`)

`failureModeAllow` is honored not only when the processor is unreachable or times out, but also when it returns a **malformed or wrong-phase verdict** — a buggy processor cannot silently bypass a configured fail-close.

- `false` (fail-close, recommended for authz-like use): short-circuit with `500 ext_proc unavailable`.
- `true` (fail-open, for non-critical enrichment): skip processing and continue to the backend.

## Limitations

- **No streamed or partial body mode**: request bodies support only full buffering.
- **No response body processing**: response body modes remain `NONE`/`SKIP`.
- **No response-header body replacement**: a response-header
  `CONTINUE_AND_REPLACE` verdict requires response-body replacement support, so
  it is treated as an unsupported verdict and follows `failureModeAllow`
  instead of silently discarding the mutation.
- **No cross-phase correlation**: each phase opens its own short-lived `Process` stream (MVP simplification), so a processor cannot carry state from the request phase to the response phase of the same transaction. Connection pooling / single-stream-per-transaction is a future optimization.
- **Response-phase `ImmediateResponse`**: custom headers are not preserved (status + body only; gRPC routes are status-only).

## Compatibility

The proto is a trimmed but wire-compatible subset of `envoy.service.ext_proc.v3`
(matching field numbers). For a buffered request, headers and body are sent on
the same short-lived `Process` stream. Request and response phases still use
separate streams.

## FAQ

### Q: Can ExtProc and ForwardAuth run on the same route?

Yes. They are independent plugins. List both in `requestPlugins` (in the order you want them to run) — e.g. ForwardAuth authenticates first, then ExtProc enriches headers.

### Q: My processor returns a header but the client/backend doesn't see it?

Check the phase: request-header mutations reach the **upstream** (backend); response-header mutations reach the **client**. Also confirm the entry is in the matching list (`requestPlugins` vs `upstreamResponsePlugins`) with the corresponding mode set to `SEND`. If the header is `Content-Length`/`Connection`/etc., it is intentionally rejected (see Protected headers).

### Q: The controller rejected my EdgionPlugins resource?

Common causes: `requestBodyMode: STREAMED`/`BUFFERED_PARTIAL`, any response body
mode other than `NONE`/`SKIP`, a `grpcService.target` that is a URL or missing a
port, or a `timeoutMs` of `0` or above `60000`.

### Q: What happens if the processor is down?

Governed by `failureModeAllow`: fail-close (`false`) returns `500`; fail-open (`true`) forwards the request unprocessed.

## Related links

- [ForwardAuth Plugin](./forward-auth.md)
- [Response Rewrite Plugin](./response-rewrite.md)
- [Filters overview](../README.md)
