---
name: fs-initial-delivery-backpressure
description: Use when reviewing FS initialization delivery, readiness, bounded kind channels, or dispatch shutdown.
knowledge_type: regression-guards
---

# FS initialization delivery and backpressure

## Conclusion and scope

FS already has one resource-controller receiver per kind. Its bounded mpsc
transport awaits room instead of overwriting older events. For each subscribed
kind, initialization delivers Init, every retained initial document, and InitDone
in FIFO order. Receiver closure fails initialization with a fixed kind-only error.
The resource controller never publishes readiness on closure before InitDone.
Readiness still means the preceding documents have completed the existing typed
processing policy, not that every input was valid or admitted.

## Root cause and correction

ARCH-093 identified broadcast(4096) initialization without suspension or recovery.
A valid configuration exceeding the ring's capacity could overwrite InitApply
while retaining InitDone. The receiver only warned about lag and then declared
its partial cache ready. A larger ring or scheduler yield cannot guarantee delivery.

`file_system/file_watcher.rs` now moves events into awaited bounded sends, with
snapshot and sender-map guards released. `resource_controller.rs` handles the
single-consumer optional receive result; no lag recovery state remains. The
startup-only subscription asserts once per kind per generation, avoiding silent
replacement. Existing phase, empty-kind, reload, collision, structured-envelope,
key-enqueue, inline status IO and single-worker completion ownership remain.

Runtime dispatch selects shutdown against the entire dispatch future, including
blocked sends. Cancellation exits that generation; registry changes made before
cancellation are not rolled back. Reload primes a complete new registry generation.
Runtime receiver closure returns a metadata-only error. Payload-bearing SendError
values are discarded rather than logged or attached to the error chain.

## Costs and limits

One slow kind delays the central dispatcher and other kinds. Raw notify ingress
is still unbounded and can accumulate; the scan snapshot retains unconsumed
content. This establishes no total-memory/byte bound or throughput improvement.
The accepted prime-to-runtime-baseline gap can grow during backpressure.
Phase readiness waits begin after dispatch, while the outer lifecycle readiness
deadline is separate. There are no new send deadlines, acknowledgements, worker
supervision or universal shutdown deadlines. PollWatcher setup, blocking IO and
existing worker cleanup retain their limits.

## Verification and re-evaluation

Mirrored watcher tests cover delivery above production capacity, observed full
queue backpressure, guard release, receiver closure at Init/document/InitDone,
duplicate subscription, and runtime shutdown after registry mutation with a full
queue (Apply, Delete and conflict promotion). Saturation requires observing zero
sender capacity; a pending future alone can be Tokio cooperative-budget yielding.
Mirrored resource-controller tests gate the last initial document, check readiness
and cache contents before/after release, and reject closure before InitDone.
Existing phased/reload/structured-envelope and single-worker tests remain.

These are in-process regression tests, not a production incident, benchmark or
end-to-end deployment claim. Fresh command outcomes belong in the implementation
record. Re-evaluate when subscription ownership, initialization ordering, dispatch
cancellation, registry generation ownership or transport capacity policies change.

## Resolution references

- ARCH-093: lossless FS initialization delivery before readiness.
- [FS architecture](../../../01-architecture/01-controller/03-config-center/01-file-system.md)
- [Single-worker completion](single-worker-initialization.md)
