---
name: session-drop-cancellation
description: Use when reviewing federation connection shutdown, watch replacement, or blocked watch and stats sends.
knowledge_type: regression-guards
---

# Federation session drop cancels watch and stats without waiting

## Conclusion and scope

The Controller federation established-session future owns cancellation of its stats
reporter and active watch. Dropping that future must signal both tasks, including
when shutdown wins the outer select and skips normal-tail cleanup. The active
watch has its own drop guard: replacing it cancels only the previous watch.

Stats sends select cancellation against channel capacity. Dedicated Center view
sends reserve capacity with a five-second deadline, then recheck cancellation,
current policy, serving readiness and store generation before serialization/send.
If an error cannot be delivered, the session is cancelled: a failed Watch must
not leave heartbeats advertising a healthy connection. The main receive loop and
its in-flight message handler both select this session cancellation, including
when a Ping response is blocked on the same full channel. Already queued frames
cannot be revoked; Center must isolate sessions/requests and reauthorize reads.

## Rationale and evidence

ARCH-084 identified that ordinary CancellationToken destruction does not cancel
cloned task tokens. An idle watch retained the receiver and configuration manager
after cooperative client shutdown; no later resource event or CSS change was
guaranteed to release it. An outer cancellation select also cannot interrupt a
send awaited inside an already selected branch.

`fed_sync/fed_client/mod.rs::run` uses `until_shutdown` to own and drop the
connection future. `connect_and_run` awaits `run_session`; its stats guard and
WatchSession are installed before spawning their tasks. Normal completion drops
the WatchSession and cancels stats. Watch replacement cancels only the old child;
one producer permit prevents overlapping snapshot serialization. The dedicated
store's notification receiver carries no resource bodies or borrowed guards.

ARCH-084 originally covered the ConfigSyncServer reverse-watch converter. That
federation data source has been replaced by CenterWatchStore; Gateway's ordinary
configuration-sync converter remains independent. The cancellation requirement
and tests now exercise the dedicated view while retaining the original shutdown,
EOF/error, blocked-send and lifecycle-replacement scenarios.

## Alternatives and limits

Shutdown does not join descendants, drain a channel, or wait for a timeout.
Cancellation has synchronous signaling cost; task/resource destruction is eventual
under scheduling, not guaranteed complete at parent return. The supervisor's
existing ten-second top-level timeout and hung-client detach exception remain
unchanged. HttpProxy task lifetime and session-end metric behavior on future drop
are outside this correction. No claim of zero post-stop transmission is made.

The accepted process-lifetime background-cache tradeoff in
[orphaned background tasks](../../concurrency/intentional-design/orphaned-bg-task-accepted.md)
does not cover reconnect/session-owned federation watches.

## Verification and re-evaluation

The private established-session stream seam and production-used shutdown select
permit no-network tests under `tests/unit/fed_sync/fed_client/mod_test.rs`.
Regressions cover quiet producer release, three blocked sends with the outbound
receiver kept open/full, live backpressure, replacement isolation and normal
EOF/error cleanup. The shutdown assertion polls the parent to completion before
allowing descendant cleanup; receiver release is asserted separately. Historical ARCH-084 test
negative controls removed each inner send cancellation branch independently to
verify those original regressions. This is not fresh negative-control evidence
for the dedicated CenterWatchStore transport.

Fresh command results belong to the implementation verification record. Revisit
this contract when adding session-owned tasks, new blocking sends, or changing
watch replacement, transport streaming, or shutdown ownership.

## Resolution references

- ARCH-084: session-owned federation watch/stats cancellation on future drop.
