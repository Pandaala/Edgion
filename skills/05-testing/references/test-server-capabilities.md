# test_server Capabilities

This file is used to confirm what the local-integration test backend actually provides, to avoid misdiagnosing a "missing test-backend feature" as a gateway or controller bug.

## Default listening ports

`start_all_with_conf.sh` starts `test_server` like this by default:

| Capability | Port |
|------|------|
| HTTP backends | `30001,30002,30003` |
| gRPC backends (TestService + Health) | `30021,30022,30023` |
| WebSocket | `30005` |
| TCP echo | `30010` |
| UDP echo | `30011` |
| TCP Proxy Protocol v2 | `30012` |
| Fake auth / OIDC | `30040` |
| HTTPS backend | `30051` |
| HTTPS backend mTLS | `30052` |
| Raw HTTP/1.1 transport-shape backend | `30030` |
| Raw HTTP/2 (h2c) transport-shape backend | `30031` |
| Controlled L4 duplex TCP backend | `30032` |
| Controlled L4 duplex TLS backend | `30033` |

If you run `test_server` alone, the CLI default only starts the first group of ports; the integration script passes multi-port and optional TLS/auth ports together.

## HTTP endpoints

### L4 duplex backpressure fixture

`--l4-duplex-port 30032` and `--l4-duplex-tls-port 30033` start bounded,
explicitly controlled byte-stream fixtures inside the shared test server. The
TLS listener reuses `--cert-file` and `--key-file`. They serve the canonical
`TCPRoute/DuplexBackpressure` suite, which exercises TCP, TLS termination, and
TLS passthrough through the shared gateway. Run with
`run_integration.sh -r TCPRoute -i DuplexBackpressure`.

`GET /l4-duplex/{id}` exposes active connection write/pulse counters and payload
verification state. `POST /l4-duplex/{id}/{action}` accepts `pulse`, `drain`,
`finish`, and `close`; unknown IDs/actions and full command queues fail explicitly.
The suite validates blocked sends, reverse progress beyond the idle window,
exact resumed payloads, and correlated idle-close diagnostics. The integration
runtime config enables the existing `logs/tcp.log` sink for those diagnostics.
These endpoints are test controls, not Gateway APIs.

### Basic echo and health check

- `/health`
- `/echo`
- `/headers`
- `/{*path}` catch-all

### Header / request-response related

- `/request-header-test`
- `/both-headers-test`
- `/multi-request-headers`
- `/response-header-test`
- `/auth-header-probe`

These endpoints are commonly used to verify:

- request/response header modifier
- security headers
- the headers passed through or sanitized by forward auth
- the actual header form received by the upstream

### Webhook / dynamic resolution

- `/webhook/resolve`
- `/webhook/resolve-body`
- `/webhook/healthz`

These endpoints are mainly reused by webhook / dynamic upstream / external resolver scenarios.

### Status code and delay

- `/status/{code}`
- `/delay/{seconds}`

Suitable for:

- timeout
- retry
- failure fallback
- specific status-code assertions

### Streaming responses

- `/sse?events=N&interval_ms=T` streams paced `text/event-stream` events with
  sequence numbers and server-side epoch-millisecond timestamps. Defaults are
  five events at 200 ms intervals.
- `/stream-stall/{test_id}` sends one body chunk and then stalls.
- `/stream-trickle/{test_id}/{chunks}/{interval_ms}` sends paced body chunks and
  then completes normally.

Use `/sse` for event-delivery and streaming-compression assertions. The other
two endpoints model per-try timeout behavior for generic HTTP response bodies.

### OpenAI-compatible provider fixture

- `/ai/{*path}` accepts GET and POST requests on the shared HTTP server.
- `mock_mode` selects deterministic `paced`, `split`, `missing_usage`,
  `oversized`, or `abort` streaming behavior.
- `fixture_query=preserved` supports exact query-preservation checks.

### Guardrail unary decision fixture

- `POST /guardrail/decision/pass` returns a strict v1 Pass decision.
- `POST /guardrail/decision/replace` returns one `"reviewed"` replacement for
  every logical window key in the bounded request envelope.
- `POST /guardrail/decision/reject` returns a strict v1 policy Reject.
- `POST /guardrail/decision/malformed` returns a 200 JSON content type with an
  intentionally malformed body for dependency fail-open/fail-close tests.
- `POST /guardrail/decision/timeout` records the request and waits 500 ms
  before returning Pass, for static-deadline fail-close coverage.
- `GET /guardrail/observation/{action}` returns only a call count, window
  counts, and structural validation booleans. It never retains semantic text.
- `POST /guardrail/reset` clears those bounded observations between cases.

These endpoints share the ordinary HTTP server and are intended to be paired
with `/ai/{*path}`. They retain no request content and add no process or port.
The AI provider modes `guardrail_split` and `guardrail_two_windows` emit,
respectively, one split canonical semantic event and two complete canonical
semantic events with explicit choice indices for call-count assertions.

The response exposes only fixed fixture identifiers and booleans for model,
path, Host, usage-option, and credential checks. It never echoes an incoming or
provider Authorization value. Use this endpoint for `EdgionBackend` tests;
do not start a separate mock-provider process.

### Mirror test endpoints

- `/mirror/capture`
- `/mirror/query/{trace_id}`
- `/mirror/slow/{ms}`
- `/mirror/reset`

`RequestMirror`-related tests typically:

1. Send the main request first with `x-trace-id`
2. Poll `/mirror/query/{trace_id}` to see whether the mirrored request arrived
3. Use `/mirror/reset` to clean up the scene if needed

## Transport-shape backends

Two **raw** listeners, deliberately not on the axum router: `axum::serve` builds a hyper auto-negotiating connection and cannot express half-close, a RST at a chosen request-body byte offset, connection identity claimed at `accept()`, `END_STREAM`-on-HEADERS observation, or a response whose framing contradicts itself. Wiring that machinery into the shared listener would also put it on the path of the ~40 endpoints five other suites depend on, so it lives on its own ports.

| Flag | Port | Speaks | Source |
|------|------|--------|--------|
| `--transport-h1-port` | `30030` | hand-written HTTP/1.1 over a tokio `TcpListener` | `edgion-tests/src/server/transport/h1.rs` |
| `--transport-h2-port` | `30031` | h2c prior knowledge via `h2::server` | `edgion-tests/src/server/transport/h2.rs` |

These are a **local-stack** capability only. They are deliberately not wired into `edgion-tests/k8s/**`: that would need deployment manifests, EndpointSlice IP plumbing and Service objects for ports used only by local transport-focused suites.

### Steering: `x-up-*` request headers

Set on the original request by the test client; they survive the proxy and are read from the request the gateway forwards. An unrecognised value never silently degrades — it is reported in the `x-up-script-warning` response header and in the `RespHead` event's `detail`.

| Header | Values | Meaning |
|---|---|---|
| `x-up-behavior` | see the table below | which script this request runs (default `echo`) |
| `x-up-at-bytes` | integer, default `0` | request-body byte offset at which the action fires; `0` = immediately after the headers |
| `x-up-reset-code` | `NO_ERROR`, `CANCEL`, `INTERNAL_ERROR`, `REFUSED_STREAM`, `PROTOCOL_ERROR`, … | HTTP/2 error code; unknown names fall back to `INTERNAL_ERROR` |
| `x-up-reset-first` | integer, capped at `8` | for `grpc-say-hello`, reset the first N requests carrying the same `x-trace-id`, then return a valid unary gRPC response |
| `x-up-after-response` | `linger` (default), `close`, `reset`, `goaway` | what the upstream does **after** a complete response |
| `x-up-delay-ms` | integer, capped at `10000` | delay before the response head is written |
| `x-up-status` | 3-digit code, default `200` | response status |
| `x-up-resp-len` | integer, default `16`, capped at `65536` | response body length |
| `x-up-drain` | `true` (default) / `false` | keep reading the request body after acting |
| `x-trace-id` | opaque string | the recorder key — the same idiom `/mirror/capture` already uses |

`x-up-after-response` defaults to `linger` (HTTP/1 keep-alive, HTTP/2 connection untouched) so that connection reuse is observable at all. `goaway` on HTTP/1 degrades to `close` and records the degradation.

#### `after-response: reset` on HTTP/2 — the two `h2` facts that decide the shape

"A complete response, **then** `RST_STREAM`" is only expressible on HTTP/2 under one premise, and combining it with `echo` produces a case that asserts something the wire never carries:

1. `h2`'s `Send::send_reset` returns early — emitting **no** `RST_STREAM` at all — when the stream is already closed and its send queue is flushed. The backend has just sent `END_STREAM` on its own DATA, so the stream is closed the moment the peer's `END_STREAM` has been received. `echo` drains the entire request body before responding, which guarantees exactly that: no reset can be emitted. This is RFC 9113 §8.1's premise inverted — §8.1 is about an origin answering while the client is **still uploading**.
2. `Send::send_reset` opens by calling `Prioritize::clear_queue`, which drops every frame still queued for the stream. The response frames are only *queued* by `send_response` / `send_data`; they are written by `Streams::poll_complete` in the connection task. Resetting without giving that task a turn destroys the very response the reset is supposed to follow.

So the expressible script is `x-up-behavior: early-response` + `x-up-drain: false` + `x-up-after-response: reset`: answer at `x-up-at-bytes`, leave the request stream open, and let the backend's flush barrier run before the reset. The `RespDone` event that records the reset is written **after** `send_reset`, and carries a `code` **only** when the request stream was still open — the state in which `h2` really does queue the frame. A run in which `h2` swallowed the reset records a `RespDone` with no `code` and a detail saying so, so an assertion about a reset that was sent cannot pass on it.

`edgion-tests/src/server/transport/h2/wire_proof.rs` holds all of this to the wire: it speaks HTTP/2 by hand over a `TcpStream` and asserts the frame sequence, because an `h2` *client* cannot see the difference (receiving the reset overwrites the stream state that carried the END_STREAM evidence).

### `x-up-behavior` vocabulary

| Value | Effect |
|---|---|
| `echo` | read the whole body, then respond with the framing report headers below |
| `early-response` | respond at `x-up-at-bytes`, before the request body is complete |
| `hang` | read the body, never respond; stays awake for the peer's FIN / RST_STREAM |
| `half-close` | H1: FIN the write half and keep reading. H2: complete the response, then drop the receive stream with the request body still open |
| `reset` | H1: real TCP RST (`SO_LINGER(0)` + close). H2: `RST_STREAM` with `x-up-reset-code` |
| `close` | H1: graceful close with no response. H2: `GOAWAY(NO_ERROR)` |
| `grpc-say-hello` | H2 only: drain the request and return a valid unary `test.TestService/SayHello` response with `grpc-status: 0` trailers; optionally reset the first trace-keyed requests via `x-up-reset-first` |
| `informational-then-hang-first` | H1 only: require `x-trace-id`; flush `100 Continue` and wait for cancellation on trace request 1, then return the bounded echo response on later trace requests |
| `malformed-cl-and-te` | response carries both `Content-Length` and `Transfer-Encoding: chunked`, body written chunked |
| `malformed-chunk-size-overstated` | chunk-size line claims 8 more bytes than the chunk carries |
| `malformed-chunk-size-understated` | chunk-size line claims half the bytes the chunk carries |
| `malformed-chunk-terminator-missing` | chunked body with no terminal `0\r\n\r\n` |
| `malformed-chunk-terminator-corrupt` | terminal chunk size line is not valid hex |
| `malformed-content-length-overstated` | `Content-Length` 16 larger than the bytes written |
| `malformed-content-length-understated` | `Content-Length` half the bytes written |

The `malformed-*` family exists because the request-body seam work touches request framing and a smuggling-adjacent defect was found during its review. It is **HTTP/1 only**: HTTP/2 has no chunked framing, and the `h2` crate refuses to put a self-contradictory message on the wire, so on port `30031` these serve as `echo` and say so in the `RespHead` `detail`. Every malformed response closes the connection afterwards — a response whose framing is a lie is un-delimited by construction. What was actually emitted is recorded in the `RespHead` event's `framing` field (`content-length=8+chunked`, `chunked;size-line=12;actual=4`, …), so a case asserts on the wire rather than on intent.

### Framing report response headers (`echo`)

A convenience for the happy path only — it proves what the backend *application* saw. Every claim about the gateway's wire behaviour goes through the recorder, because on the failure paths no response is ever written.

`x-up-headers-eos` (H2: END_STREAM on HEADERS; H1: neither CL nor TE present) · `x-up-req-content-length` (or `absent`, H1 only) · `x-up-req-transfer-encoding` (or `absent`, H1 only) · `x-up-req-body-len` · `x-up-req-trailers` · `x-up-http-version` · `x-up-conn` · `x-up-conn-req-seq` · `x-up-behavior-applied` · `x-up-script-warning` (only when a control header was not understood)

### Transport event recorder

The recorder lives on the ordinary HTTP router (port `30001`), so tests reach it with plain `reqwest`, exactly like the mirror endpoints:

- `GET /transport/query/{trace_id}`
- `GET /transport/reset` — call at the start of every case
- `GET /transport/dump` — everything, for failure dumps

Event kinds: `ConnAccepted` · `ReqHeaders` · `ReqData` · `ReqTrailers` · `PeerReset` · `PeerGoaway` · `PeerHalfClose` · `RespHead` · `RespDone` · `ConnClosed`. The `Peer*` kinds are things **the gateway did to the upstream** — that is the entire reason they exist. Exactly one `ReqData` per completed request body carries `end_stream: true`, including a body that was empty by declaration. `ReqTrailers` requires real trailer fields: a bare `0\r\n\r\n`, or a `Trailer:` declaration header alone, does not count. Trace-sequenced behaviors expose `trace_request_ordinal` on their events so retries can be paired with the response each attempt produced. An H2 `ReqHeaders` event may expose `grpc_timeout_value` and `grpc_timeout_unit`; the backend emits them only after validating the gRPC timeout grammar, so the recorder never retains arbitrary header text.

`/transport/query/{trace_id}` returns the trace's own events **plus** every untagged event on each connection that carried that trace, because `ConnAccepted` / `ConnClosed` happen before any trace id exists:

```json
{
  "found": true,
  "count": 7,
  "events": [
    {"seq": 2, "at_ms": 9, "listener": "h2", "conn": 0, "stream": 1, "trace_id": "t-abc",
     "kind": "ReqHeaders", "headers_eos": false, "content_length": 10, "method": "POST", "path": "/e"}
  ],
  "connections": [{"listener": "h1", "conn": 0, "requests": 2, "traces": ["t-abc", "t-def"], "closed": false}],
  "conn_accepted_total": {"h1": 1, "h2": 0},
  "dropped": 0
}
```

Because the answer is scoped to one trace, a claim about a *sibling* trace must be asked of that sibling. `TransportProbe::expect_none` queries the probe's primary trace, so forbidding an event tagged with a sibling trace there forbids something the answer can never contain — it passes whatever the gateway did; use `expect_none_for(&sibling, ..)`. The same scoping is why a case whose every request runs on a sibling trace trips the exercised-upstream guard, which also looks at the primary trace only: anchor at least one request on `probe.trace_id()`.

Connection identity is exposed three redundant ways on purpose. "The pooled connection was reused" = both trace ids in one `connections[].traces`. "The poisoned connection was not reused" = the second trace's `conn` differs **and** `conn_accepted_total` grew by exactly one. `conn_accepted_total` is global and monotonic and is **not** cleared by `/transport/reset` (which clears events and per-connection traces only), so a delta assertion cannot be satisfied by an empty log.

## Fake Auth / OIDC endpoints

Do not build another auth mock for auth-related tests; check the following first:

- `/verify`
- `/oidc/.well-known/openid-configuration`
- `/oidc/jwks`
- `/oidc/introspect`
- `/health`

These endpoints already cover:

- `ForwardAuth`
- `OpenidConnect`
- token introspection
- JWKS fetching

## Protocol-level services

- gRPC: used for `GRPCRoute` and `EdgionTls/grpctls` scenarios
- WebSocket: `/ws`
- TCP: plain echo
- UDP: plain echo
- TCP PP2: with Proxy Protocol v2 parsing capability

If your test just needs a standard echo, TLS handshake, OIDC/JWKS/mock auth, prefer reusing the existing services.

## TLS-related backends

`test_server` can also start two kinds of TLS backends if certificates are provided:

- HTTPS backend: for backend TLS / HTTPS upstream scenarios
- HTTPS backend mTLS: for mutual-TLS upstream scenarios

Certificates are usually generated by `edgion-tests/integration/scripts/gen_certs/` and then passed to `test_server` by `start_all_with_conf.sh`.

## When to extend `test_server`

Modify `edgion-tests/src/server/test_server.rs` only when:

- Existing endpoints cannot express your assertion needs
- A protocol-level capability is missing
- Existing OIDC/auth/webhook/mirror behavior cannot cover your scenario
- You need deterministic test-backend behavior rather than an online service or Docker dependency

The transport-shape backends and the event recorder live next to it in `edgion-tests/src/server/transport/` (`h1.rs`, `h1_wire.rs`, `h2.rs`, `recorder.rs`, `script.rs`); extend those instead when the missing capability is protocol- or connection-level rather than another endpoint.

Before modifying, search the existing endpoints first:

```bash
rg -n "route\\(|start_auth_server|mirror|oidc|verify|webhook" edgion-tests/src/server/test_server.rs
rg -n "x-up-|KIND_" edgion-tests/src/server/transport/
```
