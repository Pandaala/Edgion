# Shared L4 relay idle-timer reset coalescing

Status: deferred optimization decision for the user.
Source baseline: 99ce437ec. No runtime or test change.

## Evidence

The TCP, TLS-terminated and TLS-passthrough proxies all call
`routes/l4_duplex.rs::bidirectional_relay`. Its bounded poll loop gives each
direction one I/O step per sweep, alternates the first direction, and performs
at most 32 sweeps before self-waking. `Read(n)` and `Written` each call
`reset_idle`, which samples `Instant::now()` and resets the shared Sleep to
that instant plus the connection-pinned idle duration. Read accounting happens
before that sample. A completed sweep can therefore perform two resets, up to
64 per poll; EOF or errors can return partway through a sweep. The timer is
polled after both directions on every completed sweep.

`Flushed` allows another sweep but does not reset the idle clock. Pending I/O
also does not reset it. With idle disabled, reset_idle performs no clock read
or reset and the timer is not polled. A blocked write or downstream flush must
not prevent reverse-direction progress from extending the shared deadline.

Cargo.lock pins Tokio 1.53.1. Inspection of its `time/sleep.rs`,
`runtime/mod.rs` and `runtime/time/entry.rs` shows that the traditional timer
first tries atomic `extend_expiration`, then re-registers if that fails.
It is incorrect to count every reset as a timer-wheel lock/re-registration.
Sleep also updates its stored deadline and dispatches through the runtime
handle. The alternative timer is separately gated by unstable Tokio settings;
these traditional-path observations are not a claim about every configuration.
No current workload measurement establishes that reset work is significant.

## Choices and tradeoffs

- Retain the current direct progress-to-reset mapping until profiling justifies
  added state and branching.
- Benchmark coalescing resets within one sweep while retaining the timestamp
  sampled at each original progress location. Apply the last sampled deadline
  before the existing timer poll. This can remove a redundant reset when both
  directions advance, but does not eliminate the per-progress clock reads.
- Sampling once per sweep/poll or periodically is a different deadline policy.
  A timestamp sampled early can expire before the current last-progress deadline;
  one sampled late can extend it without intervening byte progress. Do not ship
  either as a mechanically equivalent optimization.

Even retaining the last sampled deadline does not by itself prove equivalence:
the driver can expire or wake a timer between the two directional I/O polls.
Review deferred-reset behavior under concurrent expiry, timer rounding, runtime
shutdown, early EOF/error and scheduler wakeups. Do not defer timer polling to
the end of all 32 sweeps or carry a pending update past a Pending/budget return.
Do not infer a throughput gain from fewer reset calls alone.

## Acceptance before implementation

Keep one canonical relay and one shared connection-pinned idle policy. Preserve
both-direction fairness, bounded 32-sweep work, self-waking, partial-write cursors,
read-based byte accounting, flush-before-next-downstream-read ordering, EOF and
all error mappings. Only reads and advancing writes extend idle; successful
flush alone does not. Preserve disabled-idle behavior and cancellation cleanup.
Do not add background pumps, timer pools, extra locks, request tracing or metrics.

Measure bounded versus disabled idle, one-way and simultaneous two-way traffic,
small/large I/O, partial writes, stalled writes/flushes and near-deadline reverse
activity. Separate reset/clock counts from allocation, CPU and throughput; use
the actual resolved Tokio runtime and repeat representative trials. Add precise
boundary tests if implementing, including progress in the first versus second
direction, no progress, expiry between polls, pending flush, early terminal exit
and budget exhaustion. Existing baselines do not prove the candidate.

Original authority: [shared relay contract](../../skills/04-review/routing/regression-guards/l4-duplex-relay-centralization.md)
and [pinned L4 timeout publication](../../skills/04-review/listeners/regression-guards/l4-timeout-snapshot-publication.md).
Fresh baseline: 21 shared-relay unit tests passed serially. No candidate
implementation, benchmark, real-network run or throughput claim accompanies this task.
