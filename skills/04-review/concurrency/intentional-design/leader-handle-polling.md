---
name: leader-handle-polling
description: Use when reviewing findings that propose replacing the `LeaderHandle` 100-200ms `AtomicBool` polling loop with `watch::channel` or `tokio::sync::Notify`; documents why the polling pattern is accepted given lease cadence and atomic load cost.
knowledge_type: intentional-design
---

# `LeaderHandle` 100–200ms Polling is Not Worth Replacing With watch::channel

**Finding pattern**: `LeaderHandle` exposes only `Arc<AtomicBool>`; three sites poll it on `tokio::time::sleep(100ms|200ms)` loops (`center.rs::run_main_flow` 200ms, `center.rs::run_all_serve_lifecycle` 100ms, `leader_election.rs::wait_until_leader*` 100ms). Reviewer proposes adding a `watch::Sender` to `LeaderElection` and a receiver to `LeaderHandle` so listeners can await transitions.

**Why not accepted**:

1. **Event rate is essentially zero**: lease cadence is `retry_period_secs=2s` / `lease_duration_secs=15s`. Steady-state transitions are minutes-to-hours apart; even worst-case flap is seconds apart.
2. **Latency is in the noise**: 100–200ms added to a transition that already takes 2–15s end-to-end. Sub-percent of the inherent lease latency.
3. **Poll cost is zero**: one `Ordering::Acquire` atomic load per tick, no syscall, no allocation.
4. **`watch` introduces subtle failure modes**: only retains the latest value, so an `acquire → lose → acquire` burst collapses to a single notification; the bidirectional watcher in `run_all_serve_lifecycle` would also need to handle the initial-value-equal-to-current-state case. Easy to write a subtle bug for a non-problem.

**Rule**: Treat "should use `watch::channel` / `Notify` instead of polling `AtomicBool`" findings on `LeaderHandle` as accepted tradeoff. Re-evaluate only if leader transitions become a frequent event or the 100–200ms gap shows up in real incident analysis.
