---
name: udp-session-generation-retirement
description: Use when reviewing UDP idle expiry, same-peer session recreation, receiver exit, and permit ownership.
knowledge_type: regression-guards
---

# UDP session retirement follows the owning generation

## Conclusion and scope

`EdgionUdpProxy` retires a `ClientSession` only while its `Arc` is still the
current value for the direct peer address. Expiry rechecks `last_activity` under
the session lifecycle lock before removal. Successful send accounting and
retirement share that lock, so activity resumed after candidate collection
prevents expiry. A retired session's receiver exits even if the same peer has a
replacement session. Receiver error and cancellation also retire their own
current session. Only the path that actually removes a session submits its
terminal UDP access-log entry.

The `OwnedSemaphorePermit` belongs to `ClientSession` and remains held while a
sender, receiver, or other owner retains that session. Removal from the map alone
cannot release a permit while its socket and receiver buffer remain live.

## Rationale and evidence

Source task `retire-udp-sessions-by-generation` identified a serving-time race:
`cleanup_expired_sessions` collected expired session Arcs, awaited terminal
logging, then removed by peer key without checking activity or identity. A send
could refresh the collected session before removal; a replacement under that
key could be removed instead. `handle_upstream_packets` checked only key
existence, so a replacement could keep the old receiver alive after the old
permit was released. The evidence was source-level concurrency analysis, not a
reproduced network exploit or measured unbounded growth.

The current implementation in
`edgion-gateway/src/routes/udp/edgion_udp.rs` uses `SessionLifecycle` for
`last_activity`, `retired`, and the four packet/byte counters. Successful sends
update counters with wrapping arithmetic under the existing lifecycle lock,
preserving the former atomic-counter overflow behavior. Terminal logging takes
a short counter snapshot under that lock and releases it before formatting or
awaiting the logger. `retire_session` checks the map value by
`Arc::ptr_eq` before marking it retired and removing it. Logging starts only
after removal; the retired session is dropped before the logging await so the
logger cannot prolong its resource ownership. The receiver checks its own
retirement state and converges on the same retirement function at exit.

## Alternatives and limits

The receiver still uses a one-second receive timeout to observe retirement
without a separate wakeup channel. An already in-flight datagram or send can
finish after retirement; its completed result does not refresh or account to a
retired session. The permit remains held until that owner and the receiver
release their Arcs. Normal process shutdown retains its existing join and
remaining-session log pass; see [UDP process shutdown](udp-process-shutdown.md).

Upstream send-result handling consumes the sender's existing session Arc after
the send completes. On failure it releases that owner before constructing and
awaiting the fixed failure log; failed sends still neither refresh activity nor
retire the map entry. Map, receiver or other owners may keep the session alive.
Successful accounting remains under the lifecycle lock. Downstream receive-loop
handling retains its session because subsequent receives still need that owner.

These local lifecycle tests do not establish UDP wire behavior or prove a
particular resource-growth rate. The existing integration scenarios are not
changed.

## Verification and re-evaluation

The tests in `edgion-gateway/tests/unit/routes/udp/edgion_udp_test.rs`
cover refreshed expiry candidates, old-generation
retirement after same-key recreation, old receiver exit despite a replacement,
permit lifetime, and a receiver already waiting when error or cancellation
arrives. `cargo test -p edgion-gateway --lib
routes::udp::edgion_udp::tests::` originally passed: 20 tests, 0 failures. This is
local lifecycle coverage without UDP datagram traffic.

The counter-ownership follow-up passed 59 UDP tests and four UDP log tests
both before and after the production change. Its added regression exercises
wrapping in both directions and proves late successful results leave retired
counters and activity unchanged. Workspace check/Clippy, formatting and the
repository guards passed. No live wire test or throughput benchmark ran for
this follow-up; it removes two atomic read-modify-write operations per accounted
successful datagram while retaining the existing mutex.

Re-evaluate if counter readers are added before retirement or producer completion,
the session map changes key or ownership model, a second receiver
is introduced, the UDP timeout becomes configurable, or terminal logging or
permit ownership moves to another task.

## Resolution references

- `retire-udp-sessions-by-generation`: confirmed serving-time defect, corrected
  with identity-aware retirement and receiver-owned resource lifetime.
