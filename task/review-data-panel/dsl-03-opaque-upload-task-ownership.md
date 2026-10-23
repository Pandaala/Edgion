# DSL opaque upload task ownership

Status: deferred for user decision. No runtime change.
Source baseline: e9a3423bb.

## Evidence and scope

In `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`,
`next_opaque_body_chunk` spawns `read_opaque_body_chunk_task` for each admitted
reader attempt, including attempts that return EOF or an error. Cancellation
can prevent spawning. This creates repeated task scheduling and ownership work;
it is not evidence of a measured throughput bottleneck or one task per transport
chunk. The preceding shared-state borrow optimization leaves this topology intact.

The task is also a cancellation boundary. `InFlightBodyReader` owns the frozen
snapshot reader while a read is pending; its unsettled drop releases the reader,
marks accounting ambiguous, clears the in-flight state and notifies settlement.
`AbortTaskOnDrop` aborts when the stream's pending read future is dropped.
`cancel_and_release_reader` registers notification before inspecting the reader
slot and aborts an in-flight task independently of another stream poll.

`call_builtin_http` settles the upload after `send().await` on both success and
failure. Its transport rationale is that an HTTP/2 response head can arrive
before upload completion while Hyper holds a chunk behind flow control and does
not poll the body again. Simply awaiting the reader inside the stream would
hide it in that unpolled future and is not an equivalent cleanup replacement.
Settlement refunds only the known unfetched reservation; ambiguous cancellation
receives no refund. Fetched-byte accounting and exact snapshot-length/EOF checks
are independent of how many bytes the peer consumes.

## Decision requested

Retain the current ownership boundary, or investigate one demand-driven serial
task per upload with a bounded handoff. The latter might amortize task creation,
but needs separate lifecycle and admission design plus measurements before
implementation. It must not prefetch, introduce parallel reads, create a private
body cache, or use an unbounded queue. Specify ownership and bounds for every
pending request, fetched chunk and queued result, including cancellation while
the consumer stops polling. A channel alone does not prove backpressure or
reader-release progress.

This decision is separate from DSL string ownership and method normalization.
No new response aggregation, configuration switch or compatibility layer is
proposed.

## Acceptance before implementation

Use IMPL with an exact source allow-list and independent plan/final reviews.
Measure task/allocation work and retained memory for resident and spilled
snapshots, varying chunk counts and stalled consumers; do not infer request
throughput from a task count. Preserve frozen generations, canonical request-body
ownership, existing deadlines, scan admission and conservative refunds.

Verify cancellation before task publication, during a pending read, after a
ready chunk with a stalled consumer, and after an early response head. Include
send failure, stream drop, EOF, truncated/empty-chunk errors, lost-wakeup races,
and reader release without another body poll. Keep exact fetched-byte accounting
and never refund ambiguous work. Exercise the real transport stall separately
from synthetic readers before claiming the transport cleanup guarantee.

At this baseline, the four `opaque_body_stream_tests` and 42 `http_call_tests`
pass. The four stream fixtures cover midstream failure accounting, pending-read
cancellation, release without another stream poll, and dropped pending futures.
They do not establish the complete proposed acceptance matrix, a real Hyper
flow-control reproduction, or a performance improvement.

Relevant contracts: `skills/01-architecture/02-gateway/dsl-engine.md`,
`skills/04-review/http-body/intentional-design/request-body-read-contract-ownership.md`,
`skills/04-review/dsl/regression-guards/bounded-body-text-reads.md`, and
`skills/04-review/dsl/regression-guards/http-call-full-body-read-before-truncate.md`.
