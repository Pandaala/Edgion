# ExtProc Compatibility and Coverage Matrix

Reviewed: 2026-08-26

This matrix records what Edgion currently proves about its HTTP ExtProc
implementation. It is an evidence index, not a claim of complete Envoy
compatibility. In particular, a protobuf field being present in Edgion's
trimmed protocol does not mean that the corresponding phase is produced or
consumed by the HTTP proxy pipeline.

## Status definitions

| Status | Meaning |
|---|---|
| Supported | Implemented, with repository evidence linked from this matrix. |
| Intentionally rejected | Excluded by validation or by an explicit proxy/body-handling invariant. |
| Not implemented | The protocol or configuration surface does not provide the behavior. |
| Blocked | A prerequisite is missing, so support must not be claimed. |

`Fast` means deterministic coverage included in the normal unit or integration
test paths. `Full` means implemented coverage selected only by `--full-test`.
`Full planned` means the scenario belongs behind that gate because it requires
long waits, stress, repeated transport combinations, certificate rotation, or
a separately built external processor. A planned row is not test evidence.

## Protocol behavior

| Capability | Status | Current evidence | Coverage |
|---|---|---|---|
| Request and final response header phases on one request-scoped `Process` stream | Supported | The default integration suite checks both mutations in [`test_basic_request_and_response_headers`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs). Exact request-header/body/response ordering and one-stream ownership are checked by [`client.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/client.rs) (`one_process_stream_carries_request_and_response_in_exact_order`) and [`attached.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/attached.rs). | Fast |
| Header collection, duplicate preservation, append actions, remove actions, and pseudo-header observation | Supported | Mutation unit tests in [`mutation.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/mutation.rs) cover overwrite, append, add-if-absent, removal, duplicate values, pseudo-headers, and raw values. | Fast |
| Mutation of pseudo-headers and proxy-managed framing/hop-by-hop headers | Intentionally rejected | [`mutation.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/mutation.rs) rejects pseudo-header mutations and proxy-managed headers; the same file has regression tests for both rules. | Fast |
| Request-header `ImmediateResponse` | Supported | [`test_immediate_response_403`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs) asserts status, body, and an allowed response-header mutation. [`client.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/client.rs) checks that `ImmediateResponse` is terminal rather than a phase verdict. | Fast |
| Request body `BUFFERED`, including body replacement, empty-to-nonempty replacement, physical-cap enforcement, and interactive `CONTINUE_AND_REPLACE` | Supported | Integration cases are in [`body_plugins.rs`](../../../../../edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs) (`ext_proc_buffered_body`, `ext_proc_creates_body_from_empty_request`, and `ext_proc_continue_and_replace_stops_body_phase`). Mutation/failure-policy unit coverage is in [`plugin.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/plugin.rs). The shared cap contract is documented with its regression evidence in [`request-body-mutation-physical-cap.md`](../../../../../skills/04-review/architecture/request-body-mutation-physical-cap.md). | Fast |
| Request body `STREAMED`, per-chunk mutation, backpressure, EOS, and mid-body `ImmediateResponse` | Supported | The ExtProc integration suite checks normal, multi-chunk, fail-open, and mid-body terminal paths in [`ext_proc.rs`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs). [`streamed.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/streamed.rs) checks replace, clear, malformed/wrong-phase verdicts, EOS, abandonment, and both failure policies. | Fast |
| Response body `STREAMED` as bounded transport windows | Supported | [`response_streamed.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs) checks lockstep pass/replace/clear behavior, the inclusive hard cap, malformed verdicts, disconnect handling, and EOS. The attached-stream integration case in [`ext_proc.rs`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs) observes response mutation and safe framing. | Fast |
| Semantic SSE-event processing by ExtProc | Not implemented | The response path deliberately exposes bounded transport windows, not semantic events. The boundary is stated in the gateway [`00-overview.md`](../../../../../skills/01-architecture/02-gateway/00-overview.md) and implemented by [`response_streamed.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs). | No claim; a `RawWindow` adapter requires separate design and tests. |
| Request `BUFFERED_PARTIAL` | Intentionally rejected | Configuration validation in [`ext_proc.rs`](../../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/ext_proc.rs) accepts only `NONE`, `SKIP`, `BUFFERED`, or `STREAMED` for request bodies. | Fast validation evidence |
| Response `BUFFERED` or `BUFFERED_PARTIAL` (sometimes shortened to `PARTIAL`) | Intentionally rejected | [`ext_proc.rs`](../../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/ext_proc.rs) rejects both response modes and has explicit validation tests. This preserves the invariant that the main upstream response is never fully aggregated. | Fast validation evidence |
| `FULL_DUPLEX_STREAMED` | Intentionally rejected | The mode is not represented by Edgion's [`BodySendMode`](../../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/ext_proc.rs), and Edgion's HTTP-body invariant permits only bounded lockstep response windows rather than full-duplex aggregation. | No claim |
| Streamed local reply after final upstream response headers are committed | Intentionally rejected | [`response_streamed.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs) treats response-body `ImmediateResponse` and header mutation through the post-commit failure policy (`header_mutation_and_immediate_follow_post_commit_policy`); it does not rebuild an already committed response. | Fast unit evidence |
| Response-header `CONTINUE_AND_REPLACE` | Intentionally rejected | [`plugin.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/plugin.rs) routes this status through `failureModeAllow` and tests the behavior in `response_header_continue_and_replace_uses_failure_policy`. | Fast unit evidence |
| Malformed, missing, duplicate, reordered, unsolicited, or wrong-phase verdict handling | Supported | Integration cases in [`body_plugins.rs`](../../../../../edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs) cover duplicate, reordered, and missing body verdicts. [`client.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/client.rs), [`plugin.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/plugin.rs), and [`streamed.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/streamed.rs) cover phase matching and failure policy. | Fast |
| `failureModeAllow` for connection, timeout, and verdict failures | Supported | The default suite checks unreachable fail-close, unreachable fail-open, bad-verdict fail-close, and streamed fail-open in [`ext_proc.rs`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs); unit cases cover both policies in [`plugin.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/plugin.rs) and [`streamed.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/streamed.rs). | Fast |

## Trailer boundary

| Capability | Status | Current evidence | Coverage |
|---|---|---|---|
| Request trailer processing by ExtProc | Not implemented | Trailer messages retain Envoy field numbers in [`ext_proc.proto`](../../../../../edgion-common/src/ext_proc/proto/ext_proc.proto), and phase matching recognizes them in [`client.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/client.rs). However, the ordinary HTTP proxy explicitly does not forward request trailers in [`proxy_http/mod.rs`](../../../../../edgion-gateway/src/routes/http/proxy_http/mod.rs), and no ExtProc request-trailer phase producer exists. | No support claim |
| Response trailer processing by ExtProc | Blocked | Trailer messages exist in [`ext_proc.proto`](../../../../../edgion-common/src/ext_proc/proto/ext_proc.proto), but no ExtProc response-trailer phase is wired into the response pipeline. The task's prerequisite, `tasks/todo/h2-trailer-eos-bypasses-response-body-filter.md`, is absent from the reviewed tree. Until that H2 trailer/EOS dependency is restored, resolved, and covered, response processing must not be described as trailer-complete. | Blocked; no support claim |
| Trailer mutation returned through `CommonResponse.trailers` or `TrailersResponse` | Not implemented | The fields are retained for wire decoding in [`ext_proc.proto`](../../../../../edgion-common/src/ext_proc/proto/ext_proc.proto), but no HTTP pipeline application path or integration assertion exists. | No support claim |

## Transport and lifecycle behavior

| Capability | Status | Current evidence | Coverage |
|---|---|---|---|
| Plaintext h2c processor connection | Supported | [`client.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/client.rs) checks channel construction and runs an in-process tonic `Process` server. The default integration processor is started by [`test_server.rs`](../../../../../edgion-tests/src/server/test_server.rs). | Fast |
| TLS processor connection with private CA verification | Supported | [`test_tls_request_and_response_headers`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs) proves TLS+h2 request and response phases using the fixture in [`05_EdgionPlugins_ext-proc-tls.yaml`](05_EdgionPlugins_ext-proc-tls.yaml). | Fast |
| TLS processor connection with `verify: false` | Supported | [`test_tls_noverify_request_and_response_headers`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs) and [`06_EdgionPlugins_ext-proc-tls-noverify.yaml`](06_EdgionPlugins_ext-proc-tls-noverify.yaml) prove the configured behavior. | Fast |
| mTLS client identity | Supported | [`test_mtls_request_and_response_headers`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs) proves a real TLS+h2 exchange in which the processor requires the runtime-generated `ext-proc-client` identity. The fixture is [`09_EdgionPlugins_ext-proc-mtls.yaml`](09_EdgionPlugins_ext-proc-mtls.yaml). This proves Edgion's own wire path, not third-party processor interoperability. | Fast |
| HTTP/1 and HTTP/2 downstream/upstream combinations for request `STREAMED` cancellation and early upstream response | Supported | The 72-case transport suite defines all four H1/H2 combinations with ExtProc `STREAMED` and exercises `streamed_upstream_early_response` plus `streamed_client_disconnect_mid_upload` in [`transport.rs`](../../../../../edgion-tests/src/client/suites/http_route/transport/transport.rs). | Fast |
| Abandonment cancels an incomplete request-side `Process` stream without fabricating EOS | Supported | [`streamed.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/streamed.rs) checks `abandoned_event_cancels_without_sending_clean_eos`; the H1/H2 transport evidence and design decision are recorded in [`abandoned-request-body-terminal-event.md`](../../../../../skills/04-review/h2-grpc/abandoned-request-body-terminal-event.md). | Fast |
| One final half-close and exactly-once attached cleanup | Supported | Final-phase half-close and post-half-close rejection are checked by [`client.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/client.rs). Attached-log finalization is idempotence-tested in [`pg_logging.rs`](../../../../../edgion-gateway/src/routes/http/proxy_http/pg_logging.rs), and response processor cleanup/cancellation is tested in [`pg_upstream_response_body_filter.rs`](../../../../../edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs). | Fast |
| Bounded concurrent processor streams and release on every terminal path | Supported | [`client.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/client.rs) checks the session limit and RAII release. [`response_streamed.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs) and [`pg_upstream_response_body_filter.rs`](../../../../../edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs) check release during response terminal arbitration. | Fast unit evidence; stress is Full planned. |
| Effective TLS hot-snapshot identity | Supported | [`client.rs`](../../../../../edgion-gateway/src/plugins/http/ext_proc/client.rs) checks that inherited TLS changes produce a distinct target runtime and that equivalent authorities share one runtime. | Fast unit evidence; repeated live certificate rotation is Full planned. |
| Long-idle processor stream | Supported | [`test_long_idle_stream_survives`](../../../../../edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs) holds the request-header verdict for three wall-clock seconds and verifies that the attached stream completes normally. The case is added only when `test_client --full-test` is selected. It does not claim transport-level keepalive probe coverage. | Full |
| Retry/discard interaction after ExtProc body handling | Not implemented as conformance coverage | The streamed request becomes non-retryable by design, but no dedicated ExtProc retry/discard transport scenario establishes the complete interaction. | Full planned |
| Repeated random chunking, slow processor, high concurrency, and abort Cartesian products | Not implemented as conformance coverage | Existing unit and fast transport cases establish bounded individual contracts, not stress or repeated scheduling evidence. | Full planned |

## Processor interoperability

The default integration service is Edgion's own deterministic fake processor in
[`test_server.rs`](../../../../../edgion-tests/src/server/test_server.rs). It is appropriate
for protocol golden tests, failure injection, and fast CI, but it cannot by
itself prove interoperability with an independently generated Envoy processor.

Envoy's official ExtProc tests likewise primarily drive fake upstreams and
mock/fake processors rather than publishing a third-party interoperability
matrix. Useful upstream references are:

- [Envoy ExtProc integration tests](https://github.com/envoyproxy/envoy/blob/main/test/extensions/filters/http/ext_proc/ext_proc_integration_test.cc)
- [Envoy streaming integration tests](https://github.com/envoyproxy/envoy/blob/main/test/extensions/filters/http/ext_proc/streaming_integration_test.cc)
- [Envoy ExtProc filter unit tests](https://github.com/envoyproxy/envoy/blob/main/test/extensions/filters/http/ext_proc/filter_test.cc)
- [Envoy mutation utility tests](https://github.com/envoyproxy/envoy/blob/main/test/extensions/filters/http/ext_proc/mutation_utils_test.cc)
- [Envoy External Processor protocol](https://github.com/envoyproxy/envoy/blob/main/api/envoy/service/ext_proc/v3/external_processor.proto)

Edgion preserves the selected Envoy package name, field numbers, and wire types
in [`ext_proc.proto`](../../../../../edgion-common/src/ext_proc/proto/ext_proc.proto).
That is structural wire-compatibility evidence only. Interoperability with a
named external processor or SDK has not yet been demonstrated and must not be
claimed as complete.

A future `--full-test` suite should build or pin an independently generated
processor, preferably from Envoy's official `go-control-plane` protobufs,
before adding a named interoperability claim. Community processors require an
explicit version pin and supply-chain review; they must not become a default
fast-test dependency implicitly.

## Test-tier policy

The default/fast path should retain deterministic, local cases:

- header/body mutations and `ImmediateResponse`;
- strict phase/EOS/verdict handling;
- fail-open and fail-close;
- representative H1/H2 early-response and cancellation shapes;
- local TLS verification, `verify: false`, and mTLS smoke tests;
- unit-level caps, concurrency ownership, cleanup, and TLS snapshot identity.

The following belong only in cases selected by the integration runner's
`--full-test` gate:

- a separately built named processor interoperability run;
- real long-idle and keepalive waits beyond the existing three-second idle smoke test;
- repeated mTLS certificate rotation;
- high concurrency and slow-processor pressure;
- repeated randomized chunk boundaries;
- expanded H1/H2 x TLS/mTLS x cancel/abort/retry combinations.

Until those cases exist and pass, this document labels them `Full planned` and
does not count them as acceptance evidence.
