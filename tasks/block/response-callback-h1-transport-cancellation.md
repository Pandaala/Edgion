# Extend response callback cancellation to H1 upload and pipeline boundaries

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-10-15 |
| Status | blocked |
| Updated | 2026-10-27 |
| Type | bugfix |
| Priority | P2 |
| Issue | Task 06 response-callback cancellation extension |

## Deferral and resume conditions

Deferred by the user on 2026-10-27. Do not start implementation without renewed
user authorization. Existing processor deadlines remain the fallback for the
incomplete-upload and pipelined-request cases; prompt disconnect-driven
cancellation is not guaranteed there.

The missing prerequisite is a reviewed Pingora-owned transport capability that
can observe closure without consuming unread request data. Extending it across
TCP/TLS, runtime event registration, connection reuse and half-close policy is
substantial work relative to these narrow scenarios. No production capacity
impact was established in this review; this is not evidence that impact is absent.

Revisit if disconnect-related resource retention materially affects concurrency
capacity, or a reusable upstream transport capability becomes available. Begin
with a bounded feasibility investigation and independent design review. An
Envoy-style early-close event is a candidate, not an approved implementation:
validate platform support, absence of busy polling, TLS behavior and the existing
`abort_on_close` policy. Do not promise immediate cancellation for every closure
case; define detectable signals and deadline fallbacks before implementation.

## AI guide summary

The bounded Task 06 repair monitors H2 reset and H1 closure after request-body
transport completion. It deliberately reuses the canonical reader, which cannot
monitor H1 close while preserving an unread upload or after it has stashed a
pipelined request byte. Design a generic Pingora-owned extension before claiming
prompt cancellation for those cases; do not consume unread bytes, misclassify a
half-close, or create an unbounded pipeline buffer.

## Evidence and remaining behavior

Locked fork `1aaa45cc70430bc782990bc20ddee3fef36d6b99` owns the downstream H1
reader and response pump. Its `read_body_or_idle(true)` consumes one idle probe
byte and stashes it for pipelining, then stays pending. During an incomplete
request body it is not a safe independent close watcher. `abort_on_close=false`
intentionally tolerates FIN so a client can still receive its response; a timeout
or later write failure can therefore remain the terminal signal.

The Gateway's transport-free response capability removes one conflicting mutable
borrow, but does not supply a detached notification for these H1 states. Existing
processor deadlines bound waits; they do not prove disconnect-driven cancellation.
This is retained work, not an accepted complete-cancellation guarantee.

## Scope and acceptance

- Independently review transport ownership and any fork API change. Retain one
  canonical reader, bounded memory, TLS semantics, H1 reuse, upload/replay,
  pipelined-byte preservation and half-close policy.
- Prove pending callback cancellation for incomplete uploads and pipelined next
  requests using real TCP/TLS. Preserve H2 stream isolation and normal SSE.
- Cover body, trailer and protocol finalization; release inflight resources once,
  prevent late guest re-entry, preserve committed status and one terminal log.
- Include upstream H1/H2 and custom/cache/retry boundaries where the chosen
  transport change applies. Do not infer coverage from the existing H1-origin
  proof alone.
- Dependency publication, manifest/lock changes and pushing require their own
  authorization. Do not patch the Cargo registry cache or silently vendor a fork.

## References

- [Current cancellation contract](../../skills/04-review/plugin-runtime/regression-guards/response-callback-downstream-cancellation.md)
- [Real transport regression](../../edgion-gateway/tests/wasm_stream_transport.rs)
