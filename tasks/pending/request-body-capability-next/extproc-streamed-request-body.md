# ExtProc Streamed Request Body

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-02 |
| Queue order | 1 |
| Status | blocked |
| Type | feature |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add bounded, request-only ExtProc `STREAMED` processing. Each downstream body
event must receive a matching verdict from every active streamed processor
before the transformed event reaches the request mirror or upstream. The
implementation must retain strict backpressure, bounded memory, protocol-safe
framing, deterministic failure behavior, and request cleanup without using the
complete-body cache to simulate streaming.

## Current blocker

The feature requires a canonical Pingora request-body transport interface that
can represent application-level continuation and terminal outcomes. The
interface must propagate a body-phase local response or intentional abort
through the H1/H2 request pumps and duplex control flow without converting it
into generic proxy failure handling. Implementation is blocked until that
interface is supplied through coordinated Pingora work.

Do not implement this feature by introducing a feature-local transport contract
or by treating a buffered body as a streamed body.

## Requirements

- Support request `processingMode.requestBodyMode: STREAMED`.
- Keep one bidirectional gRPC stream per active ExtProc instance for the request.
- Process one body event at a time in configured processor order.
- Require exactly one matching verdict for each sent body event.
- Support `CONTINUE`, chunk replacement, clearing, and body-phase
  `ImmediateResponse`.
- Apply streamed processors before RequestMirror so the mirror observes the
  same logical body as upstream.
- Preserve the existing `failureModeAllow` policy for processor failures.
- Enforce the resolved physical body cap independently for cumulative input and
  final output; cap violations are unconditional gateway failures.
- Use protocol-correct unknown-length upstream framing after mutation:
  chunked H1 and DATA-based H2 without `Transfer-Encoding`.
- Detect and reject request trailers rather than silently dropping them.
- Disable upstream retry/replay once streamed processing is selected.
- Keep the schema and registration flavor-agnostic; only the transport adapter
  may depend on the Pingora flavor.

## Scope

### In scope

- Request headers and request body on one request-scoped ExtProc stream.
- Strict per-event backpressure and bounded processor state.
- Multiple consecutive streamed ExtProc filters in configured order.
- H1 and H2 downstream/upstream combinations.
- Per-chunk mutation, local replies, cancellation, cleanup, and failure tests.
- Configuration validation, runtime wiring, documentation, and build-flavor
  verification.

### Out of scope

- `BUFFERED_PARTIAL` and `FULL_DUPLEX_STREAMED`.
- Response-body processing and request/response trailer processing.
- Request-header `CONTINUE_AND_REPLACE` in streamed mode.
- Dynamic processing-mode override or processor run-ahead queues.
- Reconnecting a failed processor stream during a request.
- Retry or replay after streamed processing has activated.

## Runtime contract

The first implementation must follow this sequence for every body event:

```text
downstream event
    -> processor A
    <- matching verdict
    -> processor B
    <- matching verdict
    -> RequestMirror
    -> upstream
```

There is no additional body queue. The request hook does not return until the
current event has completed or the request has terminated, which supplies the
backpressure boundary.

`ImmediateResponse` is a terminal outcome, not a generic hook error. If the
downstream response is uncommitted, Edgion writes the sanitized response and
stops the proxy exchange. If it is already committed, Edgion aborts or resets
the remaining streams without writing a second response.

On fail-open processor failure, the current pre-processor bytes continue past
that processor and the processor remains disabled for the rest of the request.
On fail-close, the current and later events are not forwarded. Body bytes in a
local response remain byte-exact and are never converted through lossy text
handling.

## Transport prerequisites

- Add a typed body-hook terminal action that reaches both H1 and H2 request
  pumps and the duplex join.
- Stop request-body reads and the upstream request writer on terminal action.
- Cancel the sibling upstream-response future and bypass generic retry/error
  response classification after a committed local response.
- Carry an explicit upstream body disposition into framing resolution.
- Remove `Content-Length`; use H1 chunked framing and H2 DATA end-of-stream
  framing as appropriate.
- Detect request trailers before synthesizing a successful final body event.
- Keep H1 connection reuse and H2 stream reset behavior correct after rejection.

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-gateway/src/plugins/http/ext_proc/` | Stream lifecycle, exchange state, verdict validation, mutation, and failure policy |
| `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter.rs` | Per-event processing, mirror ordering, cap accounting, and terminal outcomes |
| `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs` | Upstream body disposition and framing coordination |
| `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs` | Avoid generic error handling after a selected terminal response |
| `edgion-gateway/src/ctx.rs` | Request-scoped streamed exchange and bounded counters |
| Pingora request-body transport | H1/H2 event, trailer, cancellation, and terminal-action support |
| `edgion-tests/` | Protocol, failure, ordering, hot-reload, and build-flavor coverage |

## Subtasks

| Subtask | Status | Output |
|------|------|------|
| Canonical Pingora request-body interface | blocked | Typed continuation/terminal contract with H1/H2 behavior |
| ExtProc stream and verdict state machine | pending | One-stream-per-processor exchange and exact response matching |
| Effective-chain collection and activation | pending | Ordered streamed filter chain after backend selection |
| Body mutation, cap accounting, and framing | pending | Bounded publication and protocol-correct upstream body |
| Cancellation, local response, and retry suppression | pending | Deterministic cleanup and one-attempt semantics |
| Unit, integration, and build-flavor tests | pending | Full failure and protocol matrix |
| Documentation and independent code review | pending | Public contract, review closure, and final checks |

## Acceptance criteria

- No body event reaches upstream before all required verdicts succeed.
- Feature-owned memory remains bounded by the current event, one decoded
  verdict, fixed per-processor state, and a bounded protocol allowance.
- H1 and H2 upstream framing remains valid after arbitrary per-event mutation.
- Trailer-bearing requests are rejected rather than silently stripped.
- Input and final output remain within the resolved physical body cap.
- Timeout, disconnect, malformed response, clean close, cancellation, early
  upstream response, and `ImmediateResponse` have explicit tested outcomes.
- Configuration rejects unsupported composition before publication, and the
  Gateway repeats validation defensively.
- Schema-compatible stock builds reject execution clearly when the transport
  seam is unavailable; they never silently downgrade the mode.

## Related skills

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/05-plugin-system.md`
- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`
- `skills/03-coding/01-log-safety.md`
- `skills/05-testing/01-integration-testing.md`

## Document index

| Document | Status | Notes |
|------|------|------|
| `extproc-streamed-request-body.md` | blocked | This task record contains the current contract and blocker |
| `00-context.md` | — | Create when analysis resumes |
| `01-design.md` | — | Create after the canonical transport interface is available |
| `02-implementation.md` | — | Create at implementation start |
| `03-subtasks.md` | — | Subtasks are recorded in this task file until implementation begins |
| `06-decisions.md` | — | Create when design decisions need a separate log |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-02 | Treat the missing canonical Pingora body transport interface as a blocker | A feature-local seam would establish an unstable cross-protocol contract |
| 2026-08-02 | Use serial one-event backpressure for the first implementation | The existing asynchronous body hook can provide strict ordering without a speculative queue |
| 2026-08-02 | Keep `STREAMED` separate from complete-body `BUFFERED` processing | Arrival-time inspection and complete-document processing have different memory and lifecycle contracts |
