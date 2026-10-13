---
name: udp-session-creation-failure-cause
description: Use when reviewing UDP session admission, upstream socket initialization, failure classification, or failed-creation permit ownership.
knowledge_type: regression-guards
---

# UDP session creation preserves its failure cause

## Conclusion and scope

`EdgionUdpProxy::get_or_create_session` returns a typed `UdpStatus` failure.
Failed admission reports `SessionLimitReached`; upstream socket bind or Tokio
registration failure reports the fixed `UpstreamSocketError`. The actual packet
handler forwards that status into the existing `UdpLogEntry::failure` and
`log_udp` path. Both failures drop the datagram. No OS details, extra tracing,
metric, retry or configuration is added.

## Rationale and evidence

ARCH-046 identified two creation failures erased into `Err(())`, with the caller
unconditionally attributing both to capacity. Socket initialization could fail
after admission succeeded, so this signal was false. The owning source is
`edgion-gateway/src/routes/udp/edgion_udp.rs`; canonical serialization belongs to
`edgion-gateway/src/observe/logs/udp_log.rs`. The
[UDP architecture](../../../01-architecture/02-gateway/03-routes/05-udp-route.md)
records the two fixed categories. The
[data-plane logging rules](../../../03-coding/01-log-safety.md) apply even though
socket creation runs only for new sessions.

The private `acquire_session_permit` owns admission classification.
`finish_upstream_socket` consumes the acquired permit and the real bind result:
failure drops the permit; success returns both owned values to session
construction. No session publication or receiver spawn precedes successful
initialization. Cancellation during the bind await drops the local permit.
The failed-admission second lookup and occupied-entry RAII remain intact.
Successful sessions retain their permit until their last owner drops, as described
by [session retirement](udp-session-generation-retirement.md).

## Alternatives and limits

Reusing `UdpStatus` keeps one bounded failure vocabulary; a separate creation enum
would add another mapping. A socket factory or injection framework is unnecessary:
the production result/ownership seam admits synthetic `std::io::Result<()>` tests
without opening sockets. Existing socket fixtures are retained without expansion.
No compatibility alias is retained for the formerly incorrect capacity category;
access-log aggregations must recognize `UpstreamSocketError` separately.

The cap, family bind, established-session fast path, entry race, receiver,
retirement, pinned source, expiry and cancellation contracts are preserved. Pure
seam tests and serializer tests do not reproduce OS bind faults, establish UDP
wire behavior, or measure performance. Independent final review must inspect
the actual caller forwarding and the publication/spawn order.

## Verification and re-evaluation

Fresh targeted verification for this implementation:

- `cargo test -p edgion-gateway --lib routes::udp::`: 56 passed, 0 failed.
- `cargo test -p edgion-gateway --lib observe::logs::udp_log::tests::`:
  4 passed, 0 failed.

The new network-free tests cover exhausted/closed admission, representative
socket IO errors with one fixed cause, immediate failed-initialization permit
release, and successful
ownership until drop. UDP log tests cover both serialized categories, omitted
upstream address and zero counters. Existing lifecycle and family tests remain.

Re-evaluate if creation adds a new failure stage, moves socket initialization
after publication, changes admission ownership or second lookup, or remaps causes
at the caller. Preserve the separate
[bind-family guard](../../backends/regression-guards/session-bind-address-family.md)
and [process shutdown contract](udp-process-shutdown.md).

## Resolution references

- ARCH-046: session-creation admission and socket failures were conflated; typed
  propagation and owned-permit result handling correct that bounded contract.
