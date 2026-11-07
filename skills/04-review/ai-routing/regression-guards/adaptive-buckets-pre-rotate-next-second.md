---
name: adaptive-buckets-pre-rotate-next-second
description: Use when reviewing AI adaptive telemetry second buckets, the adaptive ticker, or proposals to change when `AdaptiveTelemetryState::tick` rotates ring slots.
knowledge_type: regression-guards
---

# Adaptive telemetry pre-rotates the next second's bucket

## Conclusion and scope

`AdaptiveTelemetryState` (`edgion-gateway/src/ai/routing/adaptive.rs`) records TTFT
successes, failure verdicts, and charged tokens into a 61-slot one-second ring.
`SecondBucket::enter(second)` accepts a write only when the slot epoch already equals
`second`; request writers never rotate or clear a slot. Therefore `tick(now)` must, after
publishing the complete windows, rotate both `now` and `now + 1`. Do not move rotation back
to "current second only, at the end of the tick", and do not align correctness to the
ticker phase.

## Root cause and fix

`run_adaptive_telemetry_ticker` uses `tokio::time::interval(1s)` anchored at task start,
so a tick fires at an arbitrary phase `p` inside each second. Before the fix, `tick(S)`
rotated only slot `S`, and only after publishing. Writes during `[S, S + p)` found the slot
still on the closed epoch `S - 61` and were silently dropped: a per-replica, process-lifetime
loss of fraction `p` (expected 50%, up to ~100%) of LowestLatency samples and failure
verdicts and of LowestUsage charged tokens. Maturity (8 samples in 30 s) then needed
`8 / (1 - p)` requests, and replicas with different phases decided differently on identical
traffic.

The fix pre-rotates `now + 1`. `rotate_to` returns early when the slot is already on the
requested second, so the following tick keeps data accepted before it. Slot `now + 1` is
the slot of `now - 60`; that second is the oldest one in the 60 s usage window and has
already been published by the same tick, and the next tick ages it out, so the ring size
stays `USAGE_WINDOW_SECONDS + 1`. The publish loop skips epochs `>= now`, so the
pre-rotated slot never enters a snapshot early.

## Limits

- A tick skipped for a whole second (`MissedTickBehavior::Skip` under a stalled runtime)
  still drops writes from the start of the unprepared second until the next tick.
- A newly created state accepts writes only after its first tick (at most one second).
- Wall-clock jumps remain outside this guard.

## Verification

`edgion-gateway/tests/unit/ai/routing/adaptive_test.rs`:
`next_second_is_writable_before_its_tick` (writes before the tick that observes the
second are kept, current second stays excluded) and
`pre_rotation_publishes_the_oldest_usage_second_before_reusing_its_slot`. Verified
2026-11-07 with `cargo test -p edgion-gateway --lib -- ai::routing` (48 passed).
Re-evaluate if the ring becomes lazily rotated on write, the window sizes change, or the
ticker gains boundary alignment.

## Deferred related work

Reset-on-backend-edit and alias/wildcard key fragmentation from the same finding are
tracked in `tasks/todo/gw-ai3-05-adaptive-telemetry-identity-and-reset.md`.

## Resolution references

- `gw-ai3-05` — phase-dependent bucket drop, fixed 2026-11-07 (feature-10-28).
