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

The stats report, watch event and ServerReload sends select cancellation against
bounded-channel capacity. Live sessions keep awaited backpressure; cancellation
may abandon an unsent message of an ended session or replaced watch. Already
queued messages are not revoked. Authentication snapshots, request IDs, versions,
stats generation checks, and normal EOF/error cleanup retain their existing order.

## Rationale and evidence

ARCH-084 identified that ordinary CancellationToken destruction does not cancel
cloned task tokens. An idle watch retained the receiver and configuration manager
after cooperative client shutdown; no later resource event or CSS change was
guaranteed to release it. An outer cancellation select also cannot interrupt a
send awaited inside an already selected branch.

`fed_sync/fed_client/mod.rs::run` uses `until_shutdown` to own and drop the
connection future. `connect_and_run` awaits `run_session`; its stats guard and
active-watch guard are installed before spawning their tasks. Normal completion
still explicitly drops the watch guard and cancels stats. The real
`ConfigSyncServer::watch` converter propagates downstream closure upstream;
`ServerCache` serialization and watcher tasks already observe receiver closure.
No cache lifecycle redesign is required.

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
allowing descendant cleanup; receiver release is asserted separately. Test
negative controls remove each inner send cancellation branch independently to
ensure its blocked-send regression actually requires that branch.

Fresh command results belong to the implementation verification record. Revisit
this contract when adding session-owned tasks, new blocking sends, or changing
watch replacement, transport streaming, or shutdown ownership.

## Resolution references

- ARCH-084: session-owned federation watch/stats cancellation on future drop.
