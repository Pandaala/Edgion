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
| Read / replace **request body** (full) | ✅ | `RequestFilter`, `requestBodyMode: BUFFERED` |
| Inspect / mutate **request body** per chunk | ✅ | `requestBodyMode: STREAMED` |
| Inspect / mutate **response body** per transport window | ✅ | `responseBodyMode: STREAMED` |
| Read / mutate real **response trailers** | ✅ | `responseTrailerMode: SEND`; H2 and representable H1.1 trailers are supported |
| Processor-selected reduction of later phases | ✅ | Header verdict `mode_override`, when explicitly allowed |

`BUFFERED_PARTIAL` request mode is rejected. Response `BUFFERED` and
`BUFFERED_PARTIAL` are rejected; the main upstream response is never aggregated.

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
          responseHeaderMode: SEND
          responseBodyMode: STREAMED
          responseTrailerMode: SKIP
        disableImmediateResponse: false
        allowModeOverride: false
        allowedOverrideModes: []
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

> One `requestPlugins` entry owns the complete request-scoped transaction. Its
> internal response companion uses the same logical Process stream; do not add a
> second ExtProc entry under `upstreamResponsePlugins`.

## Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `grpcService.target` | String | — (required) | The processor's gRPC endpoint as a bare `host:port` authority (not a URL). IPv6 must be bracketed: `[::1]:9000`. |
| `grpcService.timeoutMs` | Integer | `200` | Verdict timeout in milliseconds, `1..=60000`. Header-phase exchanges (and `BUFFERED`, whose phases share one stream) are bounded as one transaction; in `STREAMED` mode the bound applies per body-chunk exchange, and the first exchange also absorbs connect/TLS/h2 setup — raise it for TLS targets. |
| `grpcService.tls` | Object | disabled | TLS settings for the processor dial: `enabled`, `verify`, `validation.caCertificateRefs`, `clientCertificateRef` (mTLS). Absent/`enabled: false` = plaintext h2c (in-cluster default); unset sub-fields inherit the global `outboundTls` default. |
| `processingMode.requestHeaderMode` | Enum | `SEND` | `SEND` to send request headers to the processor, `SKIP` to skip. Only read by a `requestPlugins` entry. |
| `processingMode.responseHeaderMode` | Enum | `SKIP` | `SEND` / `SKIP` for final response headers on the same attached transaction. |
| `processingMode.requestBodyMode` | Enum | `NONE` | `NONE`, `SKIP`, `BUFFERED`, or `STREAMED`. In `BUFFERED` mode, request headers (`end_of_stream=false`) and the full request body (`end_of_stream=true`) are sent on one `Process` stream. In `STREAMED` mode each body chunk is exchanged with the processor as it flows, on the same stream as the header phase. `BUFFERED_PARTIAL` is rejected. |
| `processingMode.responseBodyMode` | Enum | `NONE` | `NONE`, `SKIP`, or `STREAMED`. Each bounded transport window is exchanged in lockstep; `BUFFERED` and `BUFFERED_PARTIAL` are rejected. |
| `processingMode.responseTrailerMode` | Enum | `SKIP` | `SEND` sends a phase only for real upstream trailers and never fabricates an empty message; `SKIP` leaves trailers unprocessed. |
| `disableImmediateResponse` | Boolean | `false` | When `true`, ignore an `ImmediateResponse` in any phase, apply none of its mutations, deactivate ExtProc for this request, and continue the HTTP exchange unchanged. |
| `allowModeOverride` | Boolean | `false` | Permit a processor's request- or response-header verdict to select an exact entry from `allowedOverrideModes`. Overrides can only disable later work; they cannot add capability. |
| `allowedOverrideModes` | List of processing modes | `[]` | Exact normalized allow-list for `mode_override`, with 1–8 entries when enabled. Every entry must be a reduction of `processingMode`; normalized duplicates are rejected. The list must be empty when `allowModeOverride` is false. |
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

Outgoing header values use protobuf `raw_value`, preserving non-UTF-8 bytes allowed in HTTP headers. For returned mutations, a non-empty `raw_value` takes precedence over `value`; when both are empty, the empty value is applied only with `keep_empty_value: true`. Use `raw_value` to return original bytes. Request, response, and `ImmediateResponse` header values undergo HTTP header validation; writes containing invalid bytes such as CR, LF, or NUL are skipped with an access-log diagnostic that does not include the value.

Conditional actions check whether the header name exists, including when its existing value is not UTF-8. Values of the same name retain append order; original wire order across different names is not guaranteed. Edgion applies `set_headers` in list order, followed by `remove_headers`. These action and empty-value rules describe Edgion's behavior; using Envoy's `raw_value` representation does not imply identical mutation behavior in every detail.

### Protected headers (security)

The processor **cannot** mutate:

- **Pseudo-headers** (`:method`, `:path`, `:status`, …) — preventing routing/method tampering.
- **Hop-by-hop / framing headers** — `Content-Length`, `Transfer-Encoding`, `Connection`, `Keep-Alive`, `Upgrade`, `TE`, `Trailer`, `Proxy-Connection`. These request restrictions remain unchanged. Responses additionally reject `Proxy-Authenticate` and `Proxy-Authorization` under the shared response policy; trailers retain their additional validation. The proxy owns framing and connection fields.

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

### Streamed request body

With `requestBodyMode: STREAMED`, the body is never buffered: each downstream
chunk is sent to the processor as an `HttpBody` message and the matching
`RequestBodyResponse` may mutate that chunk in place (`body` replaces the
chunk, `clear_body: true` empties it) before it is forwarded upstream. The
header phase and every chunk verdict travel on one request-scoped `Process`
stream, so the processor keeps state across the request. Back-pressure is
inherent — the gateway does not read the next downstream chunk until the
current verdict arrives.

If the primary upstream finishes its response and stops accepting the request
body before the client reaches its real end-of-stream, the gateway cancels the
request-side `Process` stream. It does not send an `HttpBody` with
`end_of_stream=true`, because the chunks already delivered to the processor
are only an incomplete prefix.

Because chunk mutation can change the total length, the upstream request uses
unknown-length framing: chunked HTTP/1.1 or HTTP/2 DATA without
`Content-Length` or `Transfer-Encoding`, and the request becomes
**non-retryable** the moment the streamed plugin is selected. A mid-body
`ImmediateResponse` terminates the request with the given status; its header
mutations are applied to the reply (protected headers rejected, as in the
request phase) and on gRPC routes the status maps to `grpc-status` trailers.
Note the upstream connection for that request is not reused. Header mutations
inside a chunk verdict are rejected as a bad verdict through
`failureModeAllow` because upstream headers are already committed.
`CONTINUE_AND_REPLACE` — at the headers phase or
mid-body — is rejected as a bad verdict in streamed mode: there is no buffered
body to replace, and "replace and stop" semantics are not implemented.
Upgrade/CONNECT requests and pre-HTTP/1.1 clients cannot be streamed-inspected
and follow the failure policy.

The gateway checks cumulative original input and cumulative final output
independently against `EdgionGatewayConfig.spec.requestBody`'s resolved physical
body cap. Input is checked before processing; output is checked after all
streamed processors and before RequestMirror or upstream publication. The cap
is inclusive. Overflow or exceeding either account terminates with HTTP 413,
even when `failureModeAllow: true` or a processor has deactivated fail-open.
Intermediate chunks remain bounded by the processor transport message limits.

Actual request trailers terminate an installed streamed chain with HTTP 400,
before a clean end-of-stream verdict can be synthesized. Both rejections cancel
incomplete processor and mirror exchanges. A response already selected by the
proxy pipeline is never replaced with a second response.

Multiple streamed processors run in configured execution order. Each waits for
its matching verdict before the next processor runs; RequestMirror receives
only the resulting final bytes. Complete-body capture/mutation and streamed
request processing cannot be combined in either order. The gateway rejects this
composition at runtime across plugin scopes: installing a streamed handler after
capture follows the processor failure policy, and subsequent capture/mutation
fails through the existing body-access policy without reading or publishing bytes.
Request-streaming policy does not enable full response buffering.

### ImmediateResponse (short-circuit)

If the processor returns an `ImmediateResponse` before the downstream response
is committed, the gateway stops proxying and returns its status and body.
Allowed custom header mutations are queued onto the fresh local reply in both
request-header and response-header phases; protected headers remain rejected.

For a canonically classified gRPC request, an explicitly present
`grpc_status` in the range 0–16 produces a trailers-only gRPC reply: HTTP 200,
the exact processor-selected `grpc-status`, and the Immediate body as the
optional encoded `grpc-message`. Presence is preserved, so status 0 is not
treated as absent. A value above 16 is a malformed verdict and follows
`failureModeAllow`; it is never clamped. For ordinary HTTP traffic,
`grpc_status` is ignored and the declared HTTP status/body path is used.

With `disableImmediateResponse: true`, the complete Immediate message is
ignored before validation: no status, body, or header mutation is applied, no
later ExtProc phase is sent, and the backend/current response continues
unchanged. Suppression is not a processor failure and does not invoke
`failureModeAllow`. An enabled Immediate response received after response
commit cannot create a second local reply and follows the post-commit failure
policy.

### Dynamic mode reduction

When `allowModeOverride` is enabled, a processor may attach Envoy's
`mode_override` to a matching request-header or response-header verdict. Edgion
normalizes the complete requested mode and accepts it only when it exactly
matches one entry in `allowedOverrideModes`.

The allow-list is reduction-only:

- request body may retain its base `BUFFERED`/`STREAMED` acquisition class or
  reduce to `NONE`; it cannot enable body access or switch acquisition class;
- response body may retain `STREAMED` or reduce to `NONE`;
- response headers and response trailers may retain their configured mode or
  reduce from `SEND` to `SKIP`;
- request-header mode is already consumed and is ignored during normalization;
  config `SKIP` body modes normalize to wire `NONE`.

For example, this base mode lets the processor choose only the listed
request-body suppression; it cannot acquire buffered access or suppress any
other phase:

```yaml
processingMode:
  requestHeaderMode: SEND
  requestBodyMode: STREAMED
  responseHeaderMode: SEND
  responseBodyMode: STREAMED
  responseTrailerMode: SKIP
allowModeOverride: true
allowedOverrideModes:
  - requestHeaderMode: SKIP       # Already consumed; normalized to SKIP
    requestBodyMode: NONE         # Allowed STREAMED -> NONE reduction
    responseHeaderMode: SEND
    responseBodyMode: STREAMED
    responseTrailerMode: SKIP
```

An override received while disabled, or a structurally valid but unlisted or
expanding override, is ignored. Malformed enum/structure in an enabled header
override is a bad verdict. Standalone overrides and body/trailer-phase
overrides are not supported.

### Response trailers

This behavior is available in ordinary Edgion builds through the pinned Edgion
Pingora revision.

With `responseTrailerMode: SEND`, a real upstream trailer map is sent as the
final `response_trailers` request on the same Process stream. If the upstream
ends without trailers, Edgion cleanly closes the stream without fabricating an
empty trailer phase. `SKIP` leaves real trailers unprocessed.

Trailer mutation preserves duplicate raw values and commits atomically: the
gateway validates a cloned map first, so fail-open never exposes a partial
mutation. Both the observed input and fully mutated result are capped at 256
fields and 64 KiB of raw name/value bytes. Pseudo-headers, `host`,
`proxy-authenticate`, `proxy-authorization`, and proxy-managed framing or
hop-by-hop names (`connection`, `keep-alive`, `proxy-connection`,
`transfer-encoding`, `content-length`, `te`, `upgrade`, and `trailer`) are
forbidden. A gRPC response must finish with exactly one valid `grpc-status`
from 0 through 16; removal is allowed only for non-gRPC responses.

H2 and custom transports may introduce a valid new trailer name. On an H1.1
downstream, a new name must already appear in the committed `Trailer`
declaration; an observed name may still be changed or removed. H1.0 cannot
represent response trailers: fail-open drops them at the transport boundary,
while fail-close terminates the connection. Trailer processing never buffers
or aggregates the response body.

### Failure handling (`failureModeAllow`)

`failureModeAllow` is honored not only when the processor is unreachable or times out, but also when it returns a **malformed or wrong-phase verdict** — a buggy processor cannot silently bypass a configured fail-close.

- `false` (fail-close, recommended for authz-like use): short-circuit with a generic `500` response.
- `true` (fail-open, for non-critical enrichment): skip processing and continue to the backend.

## Limitations

- **No partial body mode**: `BUFFERED_PARTIAL` is rejected.
- **No full-duplex mode**: `FULL_DUPLEX_STREAMED` is rejected.
- **No response aggregation**: response `STREAMED` processes one bounded
  transport window at a time and does not treat a transport chunk as a semantic
  SSE event.
- **No request trailer processing**: `responseTrailerMode` does not enable a
  request-trailer phase; streamed requests reject trailers, while ordinary proxied request trailers are not connected to
  ExtProc.
- **No response `CONTINUE_AND_REPLACE`**: ordinary body mutation is supported,
  but the terminal stop-sending status follows `failureModeAllow`.
- **No newer Envoy control surfaces**: dynamic timeout override, request drain,
  observability mode, send-body-without-waiting, and streamed ImmediateResponse
  are not part of this trimmed protocol.

## Compatibility

The proto is a trimmed but wire-compatible subset of `envoy.service.ext_proc.v3`
(matching field numbers). For a buffered request, headers and body are sent on
the same request-scoped logical `Process` stream as the final response phases.

## FAQ

### Q: Can ExtProc and ForwardAuth run on the same route?

Yes. They are independent plugins. List both in `requestPlugins` (in the order you want them to run) — e.g. ForwardAuth authenticates first, then ExtProc enriches headers.

### Q: My processor returns a header but the client/backend doesn't see it?

Check the phase: request-header mutations reach the **upstream** (backend), while response-header mutations reach the **client**. The complete ExtProc configuration belongs in one `requestPlugins` entry. If the header is `Content-Length`/`Connection`/etc., it is intentionally rejected (see Protected headers).

### Q: The controller rejected my EdgionPlugins resource?

Common causes: `requestBodyMode: BUFFERED_PARTIAL`, response `BUFFERED` or
`BUFFERED_PARTIAL`, an invalid/expanding/duplicate `allowedOverrideModes`
entry, a non-empty override list while `allowModeOverride` is false, a
`grpcService.target` that is a URL or missing a port, or a `timeoutMs` of `0`
or above `60000`.

### Q: What happens if the processor is down?

Governed by `failureModeAllow`: fail-close (`false`) returns `500`; fail-open (`true`) forwards the request unprocessed.

## Related links

- [ForwardAuth Plugin](./forward-auth.md)
- [Response Rewrite Plugin](./response-rewrite.md)
- [Filters overview](../README.md)

## Response header policy

Response, trailer, and local reply mutations use the shared policy, including `Proxy-Authenticate` and `Proxy-Authorization`. Request-header restrictions remain unchanged. Trailer-specific validation, including gRPC status requirements, still applies.

[Response header policy](../gateway-api/response-header-modifier.md#response-header-policy).
