---
name: etcd-leader-diagnostic-supervision
description: Keep Etcd candidate identity discovery outside winner authority publication and term supervision.
knowledge_type: regression-guards
finding_id: ARCH-081
---

# Etcd leader diagnostic supervision

## Trigger and correction

ARCH-081 identified a diagnostic Leader RPC between publishing `is_leader=true`
and starting the guard and term supervisor. Dedicated election connections omit
a unary deadline because campaign may legitimately wait indefinitely. A pending
Leader RPC could therefore delay shutdown, driver-exit handling, renewal expiry,
local demotion and cancellation of the published term token. The separate fresh
lease write gate cannot clear the leadership flag or cancel that token.

`EtcdLeaderElection` now retains the exact cold `LeaderInfo` serialized into its
campaign value. `publish_term` publishes that local identity and the fresh token
before the flag. `one_term` immediately starts the guard and calls
`supervise_term`, with no diagnostic network operation in that interval.
`supervise_term` resolves local demotion and term-token cancellation before
returning to asynchronous cleanup.

## Diagnostic ownership and failure semantics

The follower refresher remains bounded and reuses its session while responses
are healthy; malformed payloads preserve the prior cache, while vacancy,
transport failure or timeout discard the session for the next tick. It is an
owned future selected alongside campaign, not a spawned writer. Returning from
`campaign_with_refresher` drops the diagnostic future, including any pending
publication, before winner publication. Synchronous cache publication either
finishes during candidacy or never executes; transport activity surviving future
drop has no Controller cache writer.

A narrow `catch_unwind` preserves the prior task's diagnostic panic isolation.
The diagnostic session is discarded; its only persistent mutation is a completed
atomic cache publication. The same pinned campaign continues with its existing
shutdown/driver-exit selection. No panic payload is added to structured logs,
and diagnostic failure does not become authority loss. This does not establish
a general panic recovery policy for election authority tasks.

## Evidence and verification boundary

Production owner:
`edgion-controller/src/conf_mgr/conf_center/etcd/leader_election.rs`.
No-network regression tests reside in
`edgion-controller/tests/unit/conf_mgr/conf_center/etcd/leader_election_test.rs`:

- `winner_publishes_cold_campaign_identity_and_fresh_token` checks the exact
  campaign identity, token slot and leadership publication.
- `candidate_diagnostic_is_dropped_before_winner_publication` coordinates pending
  and ready late diagnostic results and checks winner identity preservation.
- `candidate_exit_drops_pending_diagnostic_without_publishing` checks candidate
  terminal ownership on shutdown and lease loss.
- `diagnostic_panic_preserves_pending_campaign_and_shutdown` checks diagnostic
  failure isolation without restarting campaign.
- `supervised_term_demotes_and_cancels_before_cleanup` exercises actual timer/join/
  shutdown supervision, invalidation, cache clearing and token isolation across
  successive terms.

These tests exercise local lifecycle seams, not native Etcd transport. The five
existing ignored service tests retain normal election, takeover, overwritten-key
guard, reacquisition and sweeping coverage; executing them requires real Etcd.
No reproduced service incident, transport fault-injection result, or throughput
claim follows from the local regression tests. Current execution results belong
to the implementation verification record, not historical acceptance here.

The [local transaction-gate tradeoff](../intentional-design/etcd-leadership-gate-not-fenced-at-txn.md)
remains unchanged. Initial capability probing, session setup before campaign
selection, and best-effort resignation connection setup are separate existing
boundaries; this correction makes no broader shutdown guarantee.

## Re-evaluation

Revisit if diagnostics gain independent tasks, cache writes escape the candidate
scope, winner identity stops matching the submitted cold campaign value, or a
network await is inserted before term supervision. Future transport or fencing
changes require their own evidence and authorization.
