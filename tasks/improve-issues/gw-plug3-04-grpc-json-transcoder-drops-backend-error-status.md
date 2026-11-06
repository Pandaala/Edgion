# gw-plug3-04: GrpcJsonTranscoder replaces every backend gRPC error with a fixed body and drops grpc-message and status details

- **Category**: best-practice
- **Severity**: P3 (direction/nice)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/http/grpc_json_transcoder/response.rs:26` (`ERROR_JSON`), `:54-69` (`mapped_status`), `:259-266` (unary non-OK → `ERROR_JSON`), `:96-118` (streaming terminal error frames), `:302-334` (`on_response_header` removes `grpc-message`, `grpc-status-details-bin`)

## Summary
When the backend returns a non-OK gRPC status, the transcoder maps the code to an HTTP status (for
example INVALID_ARGUMENT → 400, NOT_FOUND → 404). It always returns the same body,
`{"error":"gRPC upstream exchange failed"}`, and strips `grpc-message` and `grpc-status-details-bin`.
REST clients therefore cannot tell *why* a call failed: which field failed validation, which resource
was not found, or what quota detail applies. The message is "upstream exchange failed" even for an
ordinary application-level 404. This turns the transcoder into a protocol bridge that cannot carry a
usable REST API contract.

## Current behavior (evidence)
- Unary: `settle()` builds `ERROR_JSON.to_vec()` for any `code != 0` (`response.rs:259-266`) and sets
  `status = mapped_status(code)`.
- Streaming: the terminal frame is
  `{"type":"error","grpcStatus":N,"message":"gRPC upstream exchange failed"}` (`:96-118`, `terminal`).
- `on_response_header` removes `grpc-message` and `grpc-status-details-bin` (`:302-334`), and trailers
  are consumed only for `grpc-status`.
- The fixed message wording describes a gateway transport failure, so REST clients and dashboards
  cannot tell genuine gateway or transport errors (502 with the same body) from deliberate application
  errors.

## Industry comparison
- **Envoy `grpc_json_transcoder`** with `convert_grpc_status: true` returns the `google.rpc.Status`
  JSON (`code`, `message`, `details`) decoded from `grpc-status-details-bin`, or from
  `grpc-status`/`grpc-message`, and maps the code to the HTTP status.
- **grpc-gateway** (`runtime.DefaultHTTPErrorHandler`) returns `{"code","message","details"}` from
  `status.FromError`.
- **Google Cloud Endpoints / ESPv2 and API Gateway** forward the `google.rpc.Status` message to REST
  clients in the same way.

In all of them, `grpc-message` is treated as backend-authored, client-facing text, not as gateway
internal detail.

## Impact
REST consumers of transcoded services lose all error semantics beyond the HTTP code. Teams must either
encode errors in successful responses or drop the transcoder for a dedicated grpc-gateway. That
undermines the purpose of the feature for real APIs.

## Suggested direction
- Add an opt-in `errorFormat: status` (default can stay `opaque`). With it, the transcoder renders
  `{"code": <int>, "message": <grpc-message, percent-decoded and length-capped>, "details": [...]}`
  from the trailers. Decode `details` from `grpc-status-details-bin` only when the descriptor set can
  resolve the `Any` types. All of this fits within the existing fixed per-exchange output budget.
- Keep gateway-originated failures (`failure()`, owner conflicts, deadline, framing) on the fixed
  opaque body, with a distinguishable message such as `"gateway transcoding error"`. Clients can then
  separate gateway faults from application errors.

## Notes
- `skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md:307`
  states "No raw grpc-message or descriptor/payload/error strings enter client errors or logs". This
  finding argues the rule is too broad for the **status-conversion** case:
  - Repo rule 9 ("fail-close error bodies never leak internal detail") targets gateway-internal
    dependency errors, addresses, and traces.
  - `grpc-message` is backend-authored and meant for the caller. On native gRPC routes Edgion already
    forwards it unchanged to gRPC clients.
  - Every reference transcoder exposes it.
  Under the proposal, the opaque default stays and the richer format is an explicit operator opt-in,
  so the existing guarantee holds unless the operator opts in.
- The admission concurrency issue for this plugin is tracked separately in `gw-plugin2-01`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P3 unchanged (feature gap behind a documented restriction).
- `grpc_json_transcoder/response.rs:26` defines the fixed `ERROR_JSON`. `settle()` emits it for any `code != 0` with `mapped_status(code)` (`response.rs:250-275`). Streaming terminals carry the same fixed message (`response.rs:96-118`).
- `on_response_header` removes `grpc-message` and `grpc-status-details-bin` (`response.rs:302-334`).
- This is documented intent: the user doc (`grpc-json-transcoder.md:90`) says "without exposing raw grpc-message", and the review guard (`grpc-json-transcoder-admission-and-outcomes.md:307`) agrees. The finding correctly proposes an opt-in, keeps the opaque default, and does not argue against the guard. That leaves it a direction or feature item, not a defect.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- `grpc_json_transcoder/response.rs` has no diff; the review guard diff only bumps the prost-reflect version note.
