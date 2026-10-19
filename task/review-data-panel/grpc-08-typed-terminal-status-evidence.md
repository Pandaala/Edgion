# Typed gRPC terminal status evidence

Status: deferred interface and ownership decision for the user.
Source baseline: 3ce6048f6. No runtime or dependency change.

## Evidence

The HTTP upstream-trailer callback in
`edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_trailer_filter.rs`
captures gRPC status evidence before mutable trailer companions execute.
`grpc_json_transcoder/response.rs::status_evidence` validates the original
HeaderMap, constructs a fresh one-entry HeaderMap for a valid code using a static
HeaderValue, and otherwise returns an empty map. The valid path allocates map
storage, not a new status payload. Invalid and missing status take the empty-map
path. This work runs at terminal trailer dispatch for transcoding, not per body
chunk or for every ordinary HTTP response.

After successful companions, `response/driver.rs::finalize_grpc_transcoding`
passes that snapshot through the shared public StreamingResponseProcessor trait's
`on_actual_upstream_trailers` method. The transcoder parses its single canonical
status again. The core finalizer and snapshot helper are crate-private, but the
trait is not private. Current production has one non-default implementation of
this method (GrpcResponseProcessor); cancellation tests also implement it.

The snapshot has semantic ownership: companions can mutate the live trailers,
but cannot manufacture original transport success. Passing the live map after
mutation is incorrect. Borrowing it across mutable companion calls does not
solve the ownership problem. The outer Option distinguishes no actual trailers
from a present empty/invalid evidence map; those cases cannot be merged when
header-end-stream and original header status participate in finalization.

## Alternatives and decision

- Retain the existing map representation until the interface scope is approved.
- Carry immutable typed evidence for trailer absence, present invalid/missing
  status, and a validated code through one canonical terminal interface. Parse
  before companions and consume the same evidence afterward. Decide whether to
  narrow the shared trait contract or place a protocol-specific typed handoff
  under the existing response driver. Avoid a second competing status parser or
  duplicate terminal owner.

The potential benefit is removing valid-status map construction and reparsing.
It requires changes across callback, driver, trait, transcoder and tests; it is
not a local clone removal. Measure those operations separately before claiming
whole-request allocation or throughput gains. This task grants no implementation
or API expansion authority and does not request new general trailer/body APIs.

## Acceptance for an approved implementation

Preserve canonical codes 0 through 16, malformed/opaque values, duplicate status,
leading-zero rejection and absent-versus-present trailers. Preserve conflicts
with header status, header-EOS-only completion, original failure despite plugin
status replacement, unary HTTP mapping and streaming terminal records.

Keep fail-close preemption, trailer-lease completion before body-lease acquisition,
downstream cancellation and incomplete cleanup ordering. Failed companions must
not publish successful transcoder output. Preserve cache abandonment for actual
trailers, final-header commit boundaries, bounded conversion ownership, status
accounting and existing access-log diagnostics. Add no per-request tracing,
metrics, response aggregation or response BodyStorage.

Use the original knowledge in
`skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md`,
`skills/04-review/h2-grpc/regression-guards/trailer-done-terminal-body-dispatch.md`
and `skills/04-review/plugin-runtime/regression-guards/response-callback-downstream-cancellation.md`.
Revalidate every consumer of the shared trait before freezing implementation scope.

## Current verification

Fresh baseline passed 27 gRPC transcoder, 11 upstream-trailer and six callback
cancellation tests (44 total), formatting check, workspace check/Clippy and
agent-doc validation. Existing unrelated Clippy warnings remain. These tests
verify the current implementation, not the proposed typed interface. No full
suite, live integration or throughput benchmark ran.
