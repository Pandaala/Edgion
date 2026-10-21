# UDP session retirement wakeup versus receive polling

Status: deferred for user decision. Inspected baseline: `f2ac5c0cb`.
No runtime change; the current polling design remains valid.

## Evidence

`routes/udp/edgion_udp.rs::handle_upstream_packets` checks its own session's
`retired` flag under the lifecycle lock, then selects process cancellation against
`timeout(Duration::from_secs(1), recv_from(...))`. A timeout restarts the loop
and checks retirement again. Active receives also restart the timeout. Each
receiver owns one reusable 65,535-byte buffer; this is not a new buffer per
packet or evidence that each timeout allocates on the heap.

An idle pending receiver therefore periodically wakes even while its session
remains active. Timer/lock/scheduling work may matter across many idle sessions,
but no measurements establish the magnitude. The listener permits at most
10,000 sessions. One second is a polling interval, not a hard scheduler or
resource-release bound: downstream sends and error logging may also be awaited.

The cleanup task separately sleeps ten seconds between cleanup passes. It
collects sessions whose activity is older than sixty seconds, then rechecks
activity and exact Arc identity while retiring the current generation. Neither
the idle expiry policy nor cleanup cadence is the candidate optimization.

## Decision and alternatives

Retain polling unless profiling justifies a generation-owned retirement wakeup.
A per-session notification or cancellation primitive could let a pending receive
wait for data, global shutdown or retirement without the periodic timeout. This
adds resident state, registration and wakeup work; compare memory, idle CPU,
active traffic cost and retirement resource-release latency at realistic loads.
Do not assume that fewer timer wakeups imply a net improvement.

A selected design must prove no lost wakeup between the lifecycle check and wait
registration, including retirement before the receiver starts. The retired flag
and map identity remain authoritative. A notification must address the exact
session generation, never a replaceable peer key or a shared listener flag.
Define whether a simultaneous packet and retirement favor the same outcome as
today; faster wakeup is an intentional timing difference requiring review.

Today an already in-flight datagram or downstream send can finish after
retirement; late success does not refresh activity or counters. Adding retirement
to every await would silently change that contract. Keep the optimization scoped
to receive waiting unless a broader delivery/cancellation change is explicitly
selected. Do not hold a lifecycle or DashMap guard across an await or notifier
protocol that creates a lock-order cycle.

## Acceptance

- Preserve refreshed-expiry protection and exact-generation removal. A retired
  receiver must exit even after the same peer key is recreated, and must not
  remove, cancel, or log the replacement generation.
- Keep the permit on ClientSession until all owners release it. Socket, buffer
  and live send ownership must not outlive a prematurely refunded permit.
- Only the successful map-removal owner emits the terminal session record.
  Existing per-send failure records are separate and remain unchanged. Preserve
  counter freezing, pinned upstream-source checks and established-session bypass.
- Keep global cancellation priority, child-task tracking/joining, forced-abort
  behavior and shutdown logging boundaries unchanged. Do not introduce detached
  retirement tasks, per-packet logs or a new observability mechanism.
- Exercise retirement before registration, during a pending receive, concurrently
  with a packet/send, after activity refresh, and across same-key replacement.
  Verify pending receiver destruction and permit conservation with deterministic
  synchronization; timing sleeps alone do not establish race correctness.

The existing retired-generation test retires the session before entering the
receiver. Waiting-receiver tests cover socket errors and global cancellation;
they do not establish the proposed retirement notification's lost-wakeup safety
or timing. A future implementation needs its own reviewed plan and verification.

Original rules: `skills/04-review/listeners/regression-guards/udp-session-generation-retirement.md`,
`udp-process-shutdown.md` in the same directory, and
`skills/04-review/cpu-memory/regression-guards/udp-established-session-route-pipeline-per-packet.md`.
Fresh formatting, workspace check/Clippy and agent-doc validation passed for this
documentation batch; existing unrelated warnings remain. No Rust tests, UDP wire
scenario or benchmark ran. Historical verification is not fresh performance or
notification-correctness evidence.
