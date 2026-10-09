# ExtProc Streamed Request Body

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-02 |
| Updated | 2026-10-07 |
| Queue order | 1 |
| Status | completed |
| Type | feature |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Request-side ExtProc STREAMED and its canonical Pingora transport interface are
implemented. The reviewed implementation and fresh verification close three acceptance gaps:
independent cumulative input/output physical caps, actual request-trailer
rejection, and evidence for ordered multi-processor/mirror publication. Bounded
DSL XML remains a separate pending design task.

## Current contract

- One request-scoped bidirectional gRPC stream per active processor; serial
  matching verdicts, without processor run-ahead or full-body streaming emulation.
- Replace/clear/CONTINUE and pre-commit byte-exact ImmediateResponse are supported.
- Cumulative original input and final chain output are independently capped at
  the request-pinned physical body limit, inclusive; overflow/excess is fixed 413
  even on fail-open deactivation. Final output is checked before mirror/upstream.
- Actual trailers on handler-selected requests are rejected with 400 before
  fabricated clean EOS. Ordinary trailer discard behavior is unchanged.
- Processors run in execution order before RequestMirror. The mirror sees the
  same final chunks as upstream. Cancellation must not certify a partial upload.
- Installing a streamed handler consumes replayability monotonically; H1.1 uses
  chunked framing, H2 uses DATA without Content-Length/Transfer-Encoding.
- Complete capture/mutation and streamed processing are rejected in either order
  at runtime across plugin scopes. Handler-install refusal follows ExtProc's
  existing policy; body operations use their existing access failure policy.
- Gateway rejection cancels incomplete handlers/mirror work without overwriting
  completed or initiating processor results or a pipeline-selected response.
- The Gateway uses only the supported Edgion Pingora fork and BoringSSL backend;
  stock-Pingora/body-buffering flavor acceptance is obsolete.

## Scope

### In scope

The three closure items, necessary composition/lifecycle protections, unit and
integration regression evidence, and documentation reconciliation.

### Out of scope

BUFFERED_PARTIAL, FULL_DUPLEX_STREAMED, request-trailer processing, response
aggregation, XML implementation, reconnecting/replaying active streams, and an
expanded TLS/mTLS cancellation Cartesian product. Existing response-streaming
and response-trailer capabilities remain governed by their current contract.

## Subtasks

| Subtask | Status | Evidence |
|---------|--------|----------|
| Canonical transport interface and base streamed runtime | implemented | Locked fork RequestBodyAction/RequestRelayPlan; ExtProc streamed handler |
| Independent cumulative physical input/final-output caps | completed | Common hook accounting and unconditional rejection tests |
| Actual trailer rejection and terminal cleanup | completed | Trailer hook, handler outcome preservation, four protocol-pair cases |
| Ordered processors and RequestMirror parity | completed | Gated hook test and real two-processor H1/H2 integration fixture |
| Complete-body/streamed composition defense | completed | Session adapter guards in both orders, including replay-owner creation |
| Documentation, independent review and verification | completed | Implementation record and coverage matrix |

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| [00-context.md](00-context.md) | done | Original project sources and extension points |
| [02-implementation.md](02-implementation.md) | done | Approved plan, implementation, fresh verification and final PASS |
| [Compatibility matrix](../../../edgion-tests/integration/conf/EdgionPlugins/ExtProc/COMPATIBILITY.md) | maintained | Evidence by supported behavior; planned coverage is not acceptance |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-02 | Wait for canonical Pingora body termination interface | Historical blocker; the interface is now implemented |
| 2026-10-07 | Reject trailers only for handler-selected requests | Enforce the original STREAMED requirement while preserving ordinary traffic |
| 2026-10-07 | Keep complete-body and streamed composition unsupported | Prevent skipped inspection and detached stale mirror snapshots in either order |
| 2026-10-07 | Own cap accounts at the common hook, independent of failureModeAllow | The physical cap is a gateway invariant shared by every streamed processor |

## Acceptance

Independent IMPL plan and final reviews passed on 2026-10-07. The targeted unit
checks, workspace check/clippy and supported binary build passed. ExtProc wire
verification passed 18/18 and Transport passed 169/169, including all four
trailer protocol pairs. The separate existing runner registration-guard failure
is recorded in the implementation document and is not claimed fixed.
