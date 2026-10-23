# RequestMirror snapshot transport setup

Status: deferred design and value decision for the user.
Source baseline: 40cebc00e. No runtime or dependency change.

## Evidence

`plugins/http/request_mirror/plugin.rs::run_request` creates a bounded mpsc
channel, cancellation guard/oneshot, abandonment reason and body budget for
both committed snapshots and streaming tee requests. Inside the writer it
constructs a tracked ReceiverStream before choosing the body source. A committed
snapshot selects `snapshot_stream` instead and never consumes that receiver.
The caller drops the sender, completes cancellation and detaches the writer.
The unused tracked receiver remains a local owner in the writer scope.

This is real construction work even when the queue carries no body chunks.
Locked Tokio 1.53.1 `sync/mpsc/bounded.rs::channel` calls `chan::channel`, which
constructs an Arc<Chan>; `list::channel` also allocates its initial boxed Block.
These source sites establish avoidable queue construction, not measured allocator
cost, RSS retention or throughput. Do not assume allocation scales directly with
the configured queue capacity.

Other shared state is not automatically redundant. `snapshot_stream` uses
MirrorBodyBudget to charge each independent outgoing payload, including when
the snapshot itself is disk-backed. Its local semaphore and process budget are
required. A failed `try_copy` sets BufferLimit on the shared reason; terminal
status prefers that reason over the resulting send error. Snapshot reader and
payload destruction must precede return of their respective charges.

`MirrorCancellation::complete` closes the oneshot without sending cancellation.
`wait_for_send_or_cancel` then continues awaiting the original send result.
Explicit cancellation is different: the biased select lets it win when send
completion is also ready. An armed guard's destruction records BodyAbandoned
before closing the body channel. These semantics cannot be inferred from an
unused mpsc receiver alone.

## Alternatives and decision

- Keep common setup until snapshot frequency and allocation/setup measurements
  justify extra branching and ownership types. This is a temporary deferral,
  not a claim that the queue is necessary for snapshot transport.
- Introduce a typed body-source handoff that allocates mpsc only for the tee,
  preserving the common writer, cancellation owner and terminal outcome path.
  This is the narrower candidate. Avoid independently optional sender/receiver
  states that require new impossible-state fallbacks or unwraps.
- Separately design a detached snapshot lifecycle that also omits the external
  cancellation channel. Prove which cancellation paths are unreachable and keep
  budget-exhaustion reason propagation, deadlines and cleanup explicit. Do not
  duplicate admission, send, status, logging or metrics implementations merely
  to remove setup objects.

Queue-only removal is plausibly behavior preserving; it is not declared unsafe.
The unresolved decision is the worthwhile scope and representation. Removing
the queue does not authorize removing snapshot budget/reason state, changing
the tee path or introducing another body cache. Preserve the previous shared
counter allocation and disabled-log trace-copy optimizations.

## Acceptance for an approved implementation

Compare allocations and setup time for committed memory snapshots, disk
snapshots, headers-ended requests and streaming bodies. Report the traffic mix
and separate setup savings from transport throughput. Do not turn a headers-
ended non-snapshot request into a snapshot implicitly: current streaming framing
and header handling must remain equivalent.

Preserve current committed generation, framing-header removal, independent
snapshot/replay cursors, bounded disk reads, per-chunk payload ownership,
process/instance task admission and both byte budgets. Cover success, read
failure, budget rejection, pending-read cancellation/drop and final release of
transport-held chunks. Main request processing must remain independent, and
mirror responses must still be discarded after headers without aggregation.

Cover true, false and unknown request END_STREAM, including HTTP/2 Content-Length
zero with possible later DATA; frozen-stage rejection; explicit cancellation
versus simultaneously ready completion; clean channel closure; detached task
completion; and timeout cleanup. Preserve status precedence, yielded byte/chunk
counters, log configuration and task permit release. Keep one canonical writer
and lifecycle owner rather than parallel implementations.

Existing 28 RequestMirror tests passed serially during this investigation.
They verify current behavior, not a proposed split. No prototype, allocation
measurement, full suite or live transport validation was performed.

Original knowledge:
[body budgets](../../skills/04-review/request-mirror/regression-guards/mirror-resource-budgets.md),
[committed snapshots](../../skills/04-review/request-mirror/regression-guards/request-mirror-committed-body-snapshot.md).
