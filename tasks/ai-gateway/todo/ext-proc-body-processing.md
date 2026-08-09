# ExtProc request/response body processing (BUFFERED)

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-07-22 |
| Status | pending |
| Type | feature |
| Priority | P1 — highest-leverage body consumer (transport for guard/DLP) |
| Depends on | P0 body-buffering runtime (`read_*_body` must return `Ok`) |
| Related | `docs/superpowers/specs/2026-07-18-plugin-body-read-scaffolding.md`, `plugin-body-consumers-wiring.md` |

## Why this one first

ExtProc is the **reusable external-processing transport**: the LLM guard sidecar
and the DLP/PII tasks are meant to ride on it rather than each speaking a
classifier-specific protocol. Landing BUFFERED body support here unblocks that
whole line, so it is the top-priority body consumer after the P0 runtime.

Today the plugin is **header-only**: body modes are rejected at schema/validate
time and there is no send-body path. Do **not** declare `body_requirement()`
before the send-body protocol exists — it would only fail-close per request.

## Requirements

- Implement **BUFFERED request-body** processing first; **BUFFERED response-body**
  after response buffering is available.
- One ordered bidirectional gRPC processing stream per HTTP exchange, preserving
  strict `request-headers → request-body → response-headers → response-body`
  ordering.
- Support body mutation, immediate response, timeout, and configured
  fail-open / fail-close.
- Remove or recompute `Content-Length` per the mutation semantics (see the
  scaffolding's body-mutation API gap #1 — auto-manage framing, do not leave it
  to the author like Kong's footgun).
- Enforce body-size and gRPC-message limits.
- Read the buffered body via `session.read_request_body()` /
  `read_response_body()`; the gate guarantees it is buffered before the plugin
  runs, so a read `Err` is unexpected.

## Scope

**In:** Envoy-compatible buffered request/response messages; dynamic metadata
mapped only to approved, bounded Edgion context fields; cancellation when
client/upstream/processor terminates; conformance tests against a mock processor.

**Out:** `FULL_DUPLEX_STREAMED`; async observability mode; trailers unless the
buffered contract needs them; per-chunk processor calls on the synchronous body
filter.

## Per-consumer recipe (from the wiring task)

1. Override `EdgionPlugin::body_requirement()` for ExtProc to return a
   requirement **only once the send-body path exists**, config-driven by the
   selected body mode; default fail-close.
2. In `run_*`, read via the session and drive the stream state machine.
3. Remove the current config-rejection / "not supported" stubs.
4. Factor the pure protocol/mutation logic out of the session-reading wrapper so
   it is unit-testable without a mocked `Bytes` return.

## Acceptance

- Existing header-only behavior stays compatible.
- Each supported body mode has real behavior; unsupported modes fail validation
  with an actionable message.
- Processor mutation / immediate rejection take effect before the corresponding
  body is forwarded.
- Timeout and transport failures follow the configured failure mode.
- No external call on the synchronous response-body hot path.

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-resources/src/resources/edgion_plugins/plugin_configs/ext_proc.rs` | Enable validated body modes |
| `edgion-common` ext-proc proto module | Protocol reuse / completion |
| `edgion-gateway/src/plugins/http/ext_proc/` | Stream state machine + body mutation |
| `edgion-gateway/src/plugins/runtime/` | Body-gate / lifecycle integration |
| `edgion-tests` | Buffered ext-proc integration scenarios |
