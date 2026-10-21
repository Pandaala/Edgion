# Evaluate cancellation during retry backoff

Status: deferred for user decision. Scope: the wait in upstream_peer, not
connection establishment or a new general transport monitor.
Source baseline: Edgion `8e03bdfe6`, locked Pingora
`1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence and decision

`pg_upstream_peer.rs` awaits tokio sleep on retry attempts after computing the
exponential delay capped at 30 seconds. Request/gRPC budgets must accommodate
the full minimum delay, and deadlines are checked again after sleep. The wait
itself does not observe downstream closure; the request context stays alive.

The existing [accepted decision](../../skills/04-review/routing/intentional-design/retry-client-disconnect-log-only.md)
keeps this behavior and explicitly allows re-evaluation when a suitable Pingora
liveness API becomes available. Its historical H1 pump timing does not measure
the current backoff window, all protocols or production resource cost.

Current `response_callback.rs` already races transport-free response work against
canonical downstream closure. The locked core Session exposes
watch_h2_stream_close without reading DATA. The H1 path uses read_body_or_idle
only after body completion and outside upgrades; tolerated FIN, pipelined probe
bytes, incomplete uploads and replay restrict what it can observe. Other paths
can remain pending. These are concrete mechanisms to evaluate, not proof that
the response-specific helper can safely be applied to retry backoff unchanged.

Decide whether to retain the accepted behavior or design bounded reuse of
canonical closure detection during this wait. Potential benefit is earlier
release of abandoned request state and avoiding a subsequent peer selection or
connect after detected closure. No workload saving or latency gain has been
measured. This task does not reverse the accepted decision or authorize a new
socket reader, background task, descriptor alias, timer, log or metric.

## Required acceptance before implementation

- Preserve the full minimum backoff for live callers, the 30-second ceiling,
  request/gRPC budget failure rules and post-wait deadline checks. Cancellation
  must not shorten the wait and then retry anyway.
- Review eligibility at the retry boundary, including request-body replay,
  incomplete uploads, H1 half-close tolerance, pipelined bytes, upgrades,
  subrequests and custom transports. H2 closure must be stream-scoped and must
  neither consume upload DATA nor disrupt sibling streams.
- Decide simultaneous sleep/closure/deadline precedence explicitly. Preserve
  canonical non-retryable downstream error classification and ordered access-log
  diagnostics; do not relabel every watcher error as ClientClosed or every
  cancellation as a timeout. Review fail_to_proxy behavior and final-write
  ownership, including already selected/committed responses.
- Verify attempt counters, retry budgets, circuit-breaker permits, endpoint
  accounting and context cleanup across cancellation and successful sleep.
  Do not refund or advance state based only on the timing of the new watcher.
- Use controlled real H1/H2 transport tests with a closed retry gate: prove
  cancellation before another peer/connect, live-caller minimum wait, EOF/RST,
  tolerated FIN, upload/replay and pipelining preservation, H2 sibling isolation,
  deadline races and exactly-once finalization. Existing response-callback tests
  are a starting point, not backoff integration acceptance.
- Measure retained request resources and avoided work separately from wait
  latency; include the live-request monitoring cost. Keep the connect window
  outside the claim unless independently designed and verified.

Use IMPL after selection. Original ownership constraints are in
[response cancellation](../../skills/04-review/plugin-runtime/regression-guards/response-callback-downstream-cancellation.md)
and [minimum retry wait](../../skills/04-review/routing/regression-guards/retry-backoff-minimum-wait-conformance.md).
This record changes no runtime code. Fresh baseline selections passed 22 tests
matching backoff_, six response-callback and seventeen fail-to-proxy tests, plus
formatting, workspace check/Clippy and document validation. These do not test a
new retry cancellation implementation; no new real-network experiment ran.
