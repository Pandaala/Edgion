# DSL Plugin

> **🔌 Edgion extension**
>
> DSL is a custom scripting plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

The DSL plugin lets you flexibly customize request and response processing logic through inline EdgionDSL scripts, without developing a standalone plugin. Scripts run inside a built-in sandbox VM, executing independently for each request. Supported operations include reading and writing request headers, rejecting requests, modifying response status codes, and returning native gRPC errors.

DSL is well-suited for scenarios such as pre-authorization checks, dynamic header injection, protocol-level routing tags, and mTLS certificate-based access control. For more complex logic, developing a dedicated plugin is recommended.

## Features at a Glance

- **Sandboxed execution**: Scripts cannot access the file system or environment variables; the runtime environment is fully controlled.
- **Outbound HTTP calls**: Scripts can call external HTTP/HTTPS services via `http.call()`, including raw pull-streaming of the opaque `req.body`, with configurable request/response limits and timeouts. Destination checks always apply. Enable `httpBlockPrivateIps` to restrict private and local addresses.
- **Bytecode pre-compilation**: Supports both `source` (configuration-time compilation) and `bytecode` (pre-compiled Base64) loading modes. Bytecode mode skips compilation overhead.
- **Configurable resource limits**: Independently limit execution steps, loop iterations, call depth, and string length.
- **Storage-transparent request bodies**: Inspect or mutate an opaque `req.body` through bounded operations without materializing disk spills into one string.
- **Configurable error policies**: Three policies — `Ignore` / `Deny` / `DenyWith` — to suit different fault-tolerance requirements.
- **Native gRPC support**: Returns standard gRPC status codes via HTTP/2 trailers (`grpc_error()`).
- **Dual-phase execution**: The request phase (before forwarding) and the response phase (after upstream reply) each run independently.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-plugin
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "header-check"
        source: |
          let token = req.header("X-Api-Token")
          if token == nil {
            return deny(403, "missing X-Api-Token header")
          }
```

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Script name (used for logging and debugging); 1–36 UTF-8 bytes |
| `source` | String | **Required** (or `bytecode`) | DSL source code |
| `bytecode` | String | **Required** (or `source`) | Pre-compiled bytecode (Base64) |
| `maxSteps` | Integer | `10000` | Maximum execution steps (`1`–`1000000`) |
| `maxLoopIterations` | Integer | `100` | Maximum loop iterations (`1`–`100000`) |
| `maxCallCount` | Integer | `500` | Maximum function call count (`1`–`100000`) |
| `maxStackDepth` | Integer | `128` | Maximum stack depth (`1`–`1024`) |
| `maxStringLen` | Integer | `8192` | Maximum runtime string bytes and raw bytes requested per Body.read call (1–1,048,576) |
| `maxListLen` | Integer | `1024` | Maximum elements produced by dynamic List constructors such as `split` (`1`–`16384`); APIs with independent hard caps keep those caps |
| `maxMapSize` | Integer | `512` | Maximum entries produced by dynamic Map constructors and projections (`1`–`4096`); fixed three-field HTTP/Redis result envelopes keep their schema, while `redis.rate_limit` decisions require at least `5` |
| `maxBodyScanPasses` | Integer | `4` | Cumulative request-body scan budget, measured in physical-body-cap passes (`1`–`16`) |
| `errorPolicy` | Enum | `Ignore` | Error policy. Allowed values: `Ignore` / `Deny` / `DenyWith` |
| `httpMaxTimeoutMs` | Integer | `5000` | Hard cap on `http.call()` timeout per request (`1`–`60000` ms) |
| `httpMaxRequestBodyBytes` | Integer | `1048576` | Maximum opaque `Body` sent by `http.call()` (`1`–`33554432` bytes; also capped by the gateway physical request-body limit) |
| `httpMaxResponseBodyBytes` | Integer | `1048576` | Maximum `http.call()` response body size (`1`–`1048576` bytes) |
| `httpBlockPrivateIps` | Boolean | `false` | Block `http.call()` to private/loopback/link-local IPs |
| `linkSysRefs` | Array\<Object\> | `[]` | List of LinkSys Redis resources. **Required** if any `redis.*` builtin appears in the script. Each `name` is the alias used as the first arg to `redis.*` calls |

\* `source` and `bytecode` require at least one value; both may be provided, with `bytecode` taking precedence. Both binaries independently validate the selected representation; see [Bytecode Mode](./bytecode.md).
† Declare every Redis alias in `linkSysRefs`. Static checking rejects provable
unknown aliases; dynamic or ambiguous expressions may instead return `ok=false`
at runtime. Every `rateLimitPolicies[].link` is checked at load time.

Use `rateLimitPolicies` (array, default `[]`, at most 64 entries) to declare
named atomic Redis policies for [`redis.rate_limit`](api-reference.md#redis-rate_limit).
Each entry specifies `name`, `link`, `namespace`, `rate`, `interval`, optional
`algorithm` (default `SlidingWindow`), and `timeoutMs` (default `500`).

## Error Policies

| Policy | Behavior |
|--------|----------|
| `Ignore` | Ignore script execution errors and continue processing the request. |
| `Deny` | Reject the request when a script error occurs (returns 500). |
| `DenyWith` | Return a custom status code and message when a script error occurs. |

Request-body failures are fixed fail-close failures regardless of
`errorPolicy`. The pre-VM body gate returns an empty response with 413 for a
body-limit failure, 408 for a capture timeout, or 500 for a refusal or other
capture failure. After VM execution starts, Body availability, storage I/O,
scan-budget, and mutation-publication failures return HTTP 500 with the generic
body `Request body processing failed`. This is an internal hard-failure
classification, not another configurable failure-policy shape.

## Execution Phases

The DSL plugin runs in two independent phases, corresponding to different fields in `EdgionPlugins`.

### `requestPlugins` — Request Phase

Executes before the request is forwarded to the upstream. In this phase you can:

- Read and modify request headers, URI, Host, and HTTP method.
- Read query parameters, cookies, path parameters, and client IP.
- Read mTLS client certificate information (`req.client_cert_cn`, `req.client_cert_fingerprint`).
- Inspect the opaque request `Body` with bounded text, byte-range, hash, and definite-JSONPath operations.
- Mutate the request body transactionally with `clear`, `append`, `prepend`, `remove`, and literal `replace`.
- Reject the request and return an HTTP error via `deny()`.
- Return a native gRPC status code via `grpc_error()`.
- Pass context values to downstream plugins via `ctx.set()`.

### `upstreamResponsePlugins` — Response Phase

Executes after the upstream response is received, before it is sent to the client. In this phase you can:

- Read the upstream response status code (`resp.status`) and response headers.
- Modify response headers (`resp.set_header()`, `resp.append_header()`, `resp.remove_header()`).
- Modify the response status code (`resp.set_status()`).
- Read context values set during the request phase (`ctx.get()`).
- Read an already-captured `req.body` left by a request-stage `RequestBodyBuffer`.

A response-stage DSL never starts request-body capture. Request-body mutation
is rejected during configuration validation outside `requestPlugins`.

The response phase is asynchronous: `http.call` and `redis.*` suspend the
script without blocking a gateway worker thread, so it also works when the
gateway runs with `server.work_stealing: false`. Because DSL may perform I/O,
it is not accepted in the synchronous `upstreamResponseFilterPlugins` stage;
such an entry is rejected during configuration validation. Asynchronous
response plugins run after every synchronous `upstreamResponseFilterPlugins`
entry of the same request (route, backend, Gateway and global levels), so a
DSL script observes headers already set by those synchronous filters. Response
handlers that `ExtProc` and `Guardrail` install from `requestPlugins`, and the
`spec.waf` response phase, also run in this asynchronous stage; at the same
level they run before the DSL entries, so a DSL script sees, and may overwrite,
their header changes. Except on a `101 Switching Protocols` response,
hop-by-hop headers (`Connection`, `Transfer-Encoding`, `Keep-Alive`, ...) are
already stripped when the asynchronous stage runs: a script cannot read them,
and any it sets are removed before the response is sent.

## gRPC Error Returns

In `requestPlugins`, `grpc_error()` sends a direct trailers-only gRPC response through the canonical terminal writer. In `upstreamResponsePlugins`, it aborts the upstream response and carries the exact gRPC code and optional message to that same writer; the upstream response is not forwarded. In both phases, the wire HTTP status is `200`, while the gRPC application status is recorded separately in the access log as `grpc_status`. The code is not reconstructed from an HTTP status.

## Documentation Navigation

| Document | Contents |
|----------|----------|
| [Language Syntax Reference](./language-syntax.md) | Type system, variables, operators, control flow |
| [API Reference](./api-reference.md) | Detailed documentation for all built-in functions |
| [Sandbox Security Model](./sandbox-security.md) | Security mechanisms, resource limits, error handling |
| [Example Collection](./examples.md) | Real-world examples and best practices |
| [Bytecode Guide](./bytecode.md) | How to use pre-compiled bytecode |

## Related Documentation

- [Filters Overview](../../overview.md)
- [Plugin Composition and References](../../plugin-composition.md)

[Authenticated query-ID lookup](query-id-lookup.md): Secret-backed publication, bounded Redis reconstruction and cache placement.
