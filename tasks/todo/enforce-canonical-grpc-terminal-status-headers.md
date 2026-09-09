# Enforce Canonical gRPC Terminal Status Headers

## Meta

- Status: pending
- Type: correctness
- Priority: P2

## Evidence

The following is a confirmed source-reachable path, not a freshly executed wire
reproduction. DSL `resp.set_header("grpc-status", "1")` and
`resp.set_header("grpc-message", "conflict")` reach request-stage
`PingoraSessionAdapter::set_response_header`, which queues arbitrary names.
Returning `grpc_error(8, "quota")` produces `ErrGrpcError`, and plugin dispatch
calls `send_grpc_error_direct`.

In `edgion-gateway/src/plugins/runtime/session_adapter.rs`, both direct and
HTTP-mapped gRPC error entry points drain queued header operations and remove
framing fields, but do not remove queued `grpc-status` / `grpc-message`.
The shared `send_grpc_trailers_only_frame` in
`edgion-gateway/src/runtime/server/error_response.rs` sends the supplied initial
header unchanged on its gRPC-Web branch, then emits canonical status/message in
the `0x80` trailer body. The installed Pingora bridge's response-header filter
does not strip the conflicting fields. Native gRPC overwrites status, but a
queued message can survive when the canonical message is absent.

The response-stage wrapper already removes both names. Its regression test
therefore does not cover these request-stage adapter paths. Do not infer which
header a particular browser/client library prefers without client-specific proof.

## Repair Boundary and Acceptance

- Independently review a plan for canonical stripping at the shared terminal
  writer boundary. Preserve permitted extra headers and queue consumption.
- Trace DSL builtin, plugin dispatch and both adapter entry points; cover
  conflicting set/append values, native absent message, and gRPC-Web initial
  headers versus decoded canonical trailer fields with local tests.
- Preserve first-response ownership, HTTP 200 versus independently selected
  exact gRPC status, delivery-failure diagnostics, framing and streaming.
- Add no competing writer, failure-policy shape, traffic tracing or sensitive
  error output. Reuse the canonical response mechanism.
- Run focused error-response, adapter and gRPC regressions. Existing integration
  tests stay unchanged; record any needed new live scenario as a separate issue.

## Related Work

[Shared response-header mutation policy](unify-response-header-mutation-policy.md)
owns broader policy unification. Coordinate overlapping writer edits, but this
concrete terminal-field correction need not wait for a global policy redesign.
