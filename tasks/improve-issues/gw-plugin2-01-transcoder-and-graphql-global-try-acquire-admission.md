# gw-plugin2-01: gRPC-JSON transcoding allows 2 concurrent exchanges per process and GraphQL plugins allow 4 concurrent evaluations; excess requests get an immediate 503

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: large-direction
- **Location**:
  - `edgion-gateway/src/plugins/http/grpc_json_transcoder/budget.rs:10` (`ADMISSION = Semaphore::new(2)`)
  - `edgion-gateway/src/plugins/http/grpc_json_transcoder/plugin.rs:137` (`Reservation::acquire()` → 503), `:201` (reservation moved into `GrpcTranscodingPlan`)
  - `edgion-gateway/src/plugins/http/grpc_json_transcoder/transport.rs:67,166,229` and `response.rs:245,281` (reservation released only at terminal settle or `clear_output`)
  - `edgion-gateway/src/plugins/http/common/graphql_request.rs:19-20,156-159,177` (`MAX_EVALUATIONS = 4`, `try_acquire` → 503, permit held across `read_request_body(..).await`)
  - `edgion-gateway/src/plugins/http/rest_to_graphql/mod.rs:158-165` (same global semaphore)

## Summary
Both bounded-conversion features use a tiny process-wide `Semaphore` with `try_acquire`. They never queue,
and they reject immediately with 503. GrpcJsonTranscoder admits **two** concurrent exchanges per
Gateway process. Each admitted exchange holds its permit for the whole upstream round trip: connect,
backend processing, and the full unary response or server stream, up to the 30 s plugin timeout. The
GraphQL family (Guard, Safelist, RateLimit, OperationRouter, RestToGraphql) shares **four**
evaluation permits per process. In both cases the gateway's concurrency ceiling for these features
is a small constant, not a function of host resources. A modest load produces 503s that look like
backend unavailability.

## Current behavior (evidence)
**gRPC-JSON transcoder**
- `budget.rs:10`: `static ADMISSION … Semaphore::new(2)`. `Reservation::acquire_from` uses
  `try_acquire_owned()` and returns `None` when no permit is free.
- `plugin.rs:137-140`: `let Some(reservation) = Reservation::acquire() else { … return reject(session, 503) }`.
  This happens before the body is even read.
- `plugin.rs:201`: the reservation moves into `GrpcTranscodingPlan::new(.., reservation, ..)`, which
  is installed in the request context (`install_grpc_transcoding`).
- The permit is dropped only by `release_reservation()`. That happens in the response processor's
  terminal settle (`response.rs:245` streaming, `:281` unary) or in `clear_output`
  (`transport.rs:229`). So the permit stays held during the upstream connect, the backend's service
  time, and response streaming. A server-streaming RPC holds it until the stream ends or the
  deadline fires (default 30 s, per the operator doc).
- Effective ceiling: 2 / (upstream latency). With a 50 ms backend that is about 40 RPS per gateway
  process for all routes and all transcoder instances combined. Two slow or long-lived streaming
  calls make every other transcoded request on the node fail with 503.

**GraphQL plugins**
- `graphql_request.rs:19-20`: `MAX_EVALUATIONS = 4`, a process-global `Semaphore`.
- `:156`: `EVALUATIONS.try_acquire()` failure → `reject(session, 503)` (`GRAPHQL_ADMISSION_BUSY`).
- `:177`: the permit is held across `session.read_request_body(None).await`. That is an async read
  of the request `BodyStorage`, which may come from a disk spill. The permit is also held across the
  synchronous parse.
- The parse itself is already hard-bounded (`edgion-resources/src/graphql_request/mod.rs:20-23`:
  64 KiB envelope, 4096 tokens, 32768 work units, recursion 64). So the semaphore does not bound
  any unbounded work. It only caps parallelism at 4 across all cores. With more than 4 Pingora
  worker threads, a 5th simultaneous evaluation is rejected even when the machine is idle.
- A route that stacks GraphqlGuard + GraphqlSafelist + GraphqlRateLimit acquires a permit three
  times per request, so the effective request concurrency is lower still.

## Industry comparison
- **Envoy `grpc_json_transcoder`**: bounded per request (`max_request_body_size`,
  `max_response_body_size`, buffer limits). There is no process-global exchange count. Global memory
  pressure is handled by the **overload manager**, which shrinks buffers and stops accepting new
  connections, rather than by a hard 2-slot admission gate.
- **Apollo Router**: bounds each operation (`parser_max_tokens` default 15 000,
  `parser_max_recursion`, `max_depth`, demand control). Parsing is not serialized behind a global
  semaphore.
- **Kong / APISIX GraphQL plugins**: per-request parse with size limits; no global admission.

## Impact
- GrpcJsonTranscoder is effectively unusable for production REST→gRPC facades. Its throughput
  ceiling is far below a single backend's, and the failures look like 503 overload.
- For GraphQL, this adds load-dependent 503 noise on any multi-core deployment. Clients and
  retries amplify it.
- Operators cannot tune either limit. The doc frames it as "configuration does not silently
  increase this limit".

## Suggested direction
1. **Transcoder**: separate the memory reservation from the exchange lifetime.
   - Size the reservation from the actual request (`len`, schema node count) instead of a fixed
     32 MiB worst case. Charge a per-message reservation on the response side when a message is
     framed, and release it when that message's output `Bytes` drop. The output-credit machinery
     (`Reservation::output`, `OutputOwner`) already does the second half.
   - Do not hold an admission permit while waiting on the upstream. If a global budget is kept, make
     it a byte budget (for example a share of the process `BodyStorage` budget). Expose it in the
     Gateway config and make it wait briefly with a deadline instead of `try_acquire`.
2. **GraphQL**: remove `EVALUATIONS` altogether. The per-request work is already bounded by
   tokens, work and recursion. If CPU fairness matters, rely on the process overload protection
   (see the deferred [process-wide HTTP request admission](../todo/gateway-http-request-admission.md) task) rather than a 4-slot global gate. At minimum, do not hold the permit across the
   body read, and parse once per request with the result shared through ctx across GraphQL plugins.
3. Recommendation: (1) + (2). Keep the per-exchange hard caps (1 MiB JSON, 512 KiB message,
   depth/node limits). They are the real safety mechanism.

## Notes
- `skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md`
  ("Memory and time ledger") documents "one process-wide 64 MiB budget … reserve 32 MiB per
  admitted exchange … admits two concurrent conversions". The operator doc
  (`docs/en/user-guide/http-route/filters/edgion-plugins/grpc-json-transcoder.md:108`) repeats it.
  The ledger justifies the 32 MiB *peak* for request reflection and response conversion. **New
  evidence**: the code holds that peak reservation across the entire upstream exchange, including
  idle upstream wait and the life of a server stream (`plugin.rs:201` → `response.rs:245/281`), where
  none of the ledger's allocations are live. The ledger's memory argument does not support the
  admission lifetime it imposes.
- GraphQL `GRAPHQL_ADMISSION_BUSY` is listed in `graphql-errors.md:19`. No review page justifies the
  value 4 or holding the permit across the body read.
- Related: [process-wide HTTP request admission](../todo/gateway-http-request-admission.md) (the missing global mechanism these
  local gates stand in for).

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- `edgion-gateway/src/plugins/http/grpc_json_transcoder/budget.rs:10,19-26`: `Semaphore::new(2)` with `try_acquire_owned`; `plugin.rs:137-140` rejects with 503 before the body is read.
- `transport.rs:166,229` and `response.rs:245,281` are the only `release_reservation` sites (terminal settle / clear_output), so the 32 MiB reservation spans the upstream wait and server streams.
- `skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md:233-235` accepts "admits two concurrent conversions", but its ledger justifies only peak conversion allocations, not holding the reservation while none of them is live; that is the new evidence. The CLAUDE.md admission requirement is about bounding heap, which a byte/lifetime-scoped reservation still satisfies.
- `edgion-gateway/src/plugins/http/common/graphql_request.rs:19-20,156-159,177`: `MAX_EVALUATIONS = 4`, `try_acquire` → 503, permit held across `read_request_body(None).await`; no review page justifies the value.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `grpc_json_transcoder/budget.rs:10` still `Semaphore::new(2)`; its only diff is a `fetch_update` -> `try_update` rename in `Reservation::output`.
- `plugins/http/common/graphql_request.rs` and `rest_to_graphql/mod.rs` have no diff.
