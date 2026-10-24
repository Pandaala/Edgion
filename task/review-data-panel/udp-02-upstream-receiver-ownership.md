# UDP upstream receiver ownership

Status: deferred for user decision. No runtime design selected.
Source baseline: `9bda10ba3`.

## Evidence and cost boundary

`EdgionUdpProxy::get_or_create_session` clones the session Arc for its receiver
task and separately clones `session.upstream_socket` for the generic receiver
argument. The session already owns that socket Arc. This adds one socket Arc
increment/decrement per newly spawned receiver, not per packet, and does not
allocate another socket or packet buffer.

`handle_upstream_packets` uses the generic `UpstreamDatagramReceiver` seam for
production sockets and controlled cancellation/error tests. On successful
generation retirement, it constructs the owned log entry, drops its packet
buffer, receiver and session, then awaits terminal logging. Replacing the socket
owner with a borrow is therefore not a one-line clone removal.

## Decision alternatives

- Retain current ownership if the one-time atomic cost does not justify changing
  the receive/retirement boundary and its verification.
- Evaluate borrowing the canonical session socket inside a receive scope while
  keeping an owned session until receive/retirement work ends. The owner must
  then be releasable before any terminal logger await.

No API or extraction is selected. Keep one canonical receive loop and efficient
static dispatch; do not add a boxed future per packet or duplicate production
logic merely to keep mock injection convenient. Do not change ClientSession's
stored socket representation, packet-buffer size, admission limits or transport
behavior as part of this candidate. No measured CPU, throughput or RSS benefit
has been established, and no allocation reduction is claimed.

## Lifetime and behavior gates

A session owns its semaphore permit. Map removal alone must not release that
permit while an in-flight sender or receiver still uses the generation. Equally,
an outer wrapper must not retain the receiver's session merely to support a
borrow across terminal logging: logger backpressure must not prolong its socket,
buffer or permit ownership. Other independent owners may legitimately remain.

Preserve generation identity checks, expiry/activity arbitration, lifecycle
locking, counters, pinned-source validation, biased cancellation and send order.
Only the path that removes the current generation logs its retirement. Keep
JoinSet/TaskTracker shutdown and remaining-session logging after producer joins.
The one-second receive timeout and its deferred wakeup alternative in UDP-01 are
separate and remain unchanged. No new tracing or metrics are needed.

Before implementation, freeze an allow-list and obtain independent IMPL plan and
final review. Existing tests cover replacement generations, cancellation, errors,
permits and source predicates; they do not prove drop timing while logging is
blocked. Add explicit lifetime evidence for the selected design with a pending
terminal logger, including a remaining sender owner, cancellation while receiving,
retirement races and shutdown. Keep test seams on the canonical production loop.
Measure session-creation cost and code/future-layout consequences before claiming
a net gain; packet-forwarding throughput alone may hide this one-time cost.

## Sources and current verification

- `edgion-gateway/src/routes/udp/edgion_udp.rs`: session spawn and receive exit.
- [Generation retirement](../../skills/04-review/listeners/regression-guards/udp-session-generation-retirement.md).
- [Source pinning](../../skills/04-review/listeners/regression-guards/udp-upstream-response-source.md).
- [Process shutdown](../../skills/04-review/listeners/regression-guards/udp-process-shutdown.md).

Fresh unchanged-source checks passed 59 UDP route/session and four UDP log tests,
formatting and workspace all-target check/Clippy with existing warnings. No
prototype, benchmark, full suite or datagram wire verification ran for this task.
