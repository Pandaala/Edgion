# B0224 — Runtime Error Response

## Scope

- Pipeline state: analysis-ready; queued, not active mutation.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/server/error_response.rs`.
- Baseline: `72454f81`.
- Analyst: `b0224_error_response_analysis`.
- Read complete response construction, actual HTTP/gRPC callers, final-status/log ownership and transport/unit-test contracts before assessing local value.
- No writes authorized. Existing integration files, external todos and user-owned handoff/WAF paths remain protected. New integration needs are deferred.

## Analysis Outcome

- Analyst recommends change at `72454f81`; target blob `0b62747d3b7f98545f5606e8c892e3ca2f04fb8f`. Plan-first because response delivery is touched.
- Six fixed HTML pages currently allocate/format a body String on HTTP rejection. A small compile-time literal template plus `Bytes::from_static` can remove that work; failure-path-only benefit, no healthy-traffic/QPS claim. Decline if the implementation needs elaborate abstraction.
- Exact proposed technical scope: target; `skills/04-review/h2-grpc/regression-guards/grpc-message-encoding.md`; `skills/04-review/h2-grpc/intentional-design/h2-grpc-sentinel-refactor-declined.md`. Preserve all nine tests; cover six exact bodies and first-owner behavior with existing duplex fixtures. Preserve bytes/status/headers/claim-before-write/EOS/error propagation/H1-H2 shutdown.
- Correct encoder and UTF-8 test comments only: spaces remain unescaped, percent is encoded, current accented/Greek fixtures are two-byte. Correct removed helper/old status ownership claims in the two owning knowledge pages while retaining their valid narrow decisions; existing historical three-frame banner needs no broad rewrite.
- D0126 records independently repairable request-stage queued gRPC status/message conflicts at the shared terminal writer. Source reachability verified across DSL, adapter, dispatch and installed Pingora bridge; not a fresh wire reproduction or client-specific interpretation claim. The canonical task is self-contained; do not mix that repair into HTML optimization.
