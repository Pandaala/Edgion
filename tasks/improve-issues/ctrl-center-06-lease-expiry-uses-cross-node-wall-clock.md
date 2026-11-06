# ctrl-center-06: Kubernetes Lease expiry is judged by comparing the holder's `renewTime` with the local wall clock

- **Category**: best-practice
- **Severity**: P3
- **Scope**: local
- **Location**: `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs:544-600` (`try_acquire_or_renew`), `:842` (`lease_expired`)

## Summary

A follower decides the lease is expired when `Utc::now() > renewTime + leaseDurationSeconds`, where
`renewTime` was written with the holder's clock. Leader safety therefore depends on inter-node clock
synchronization. client-go deliberately avoids this by recording the local monotonic time at which it
last *observed* the lease record change and expiring relative to that.

## Current behavior (evidence)

- `is_expired = lease_expired(renew_time, lease_duration, Timestamp::now())`, i.e. `now - renewTime > leaseDuration` on the local wall clock (`leader_election.rs:552-556,842-844`).
- If a follower's clock is ahead by more than `lease_duration - retry_period` (15 s − 2 s by default)
  it treats a healthy lease as expired and wins the resourceVersion CAS; the old leader only learns on
  its next tick (up to `retry_period`), so both act as leader for that window (in leader-only mode both
  serve conf_sync and run leader jobs).
- Conversely, a follower whose clock is behind delays takeover after a real leader death by the skew
  amount; a leader that is itself ahead makes every follower wait `lease_duration + skew`.
- Every spurious takeover in leader-only mode costs a full epoch rebuild and a relist of every kind on
  every gateway.

## Industry comparison

client-go `leaderelection`: "it does not depend on clock synchronization between nodes; it uses the
local time at which the record was last observed" (`observedTime` + `observedRecord`, expiry =
`observedTime + LeaseDuration`). The etcd backend in this repo already uses a server-side lease TTL,
which has no such dependency.

## Impact

Rare in NTP-synchronized clusters, but the failure mode (split leadership, failover storms, or
inflated failover time) is silent and the remedy is cheap and standard.

## Suggested direction

Track `(observed_record_identity, observed_resource_version, observed_at: Instant)` locally; consider
the lease expired only when the record has not changed for `lease_duration` of local monotonic time.
Keep the resourceVersion CAS as is.

## Notes

Checked `concurrency/regression-guards/leader-lease-release-on-shutdown.md`,
`leader-pod-label-tick-resync.md`, `leader-handle-polling.md` and the HA-mode architecture page
(renewal deadline is monotonic on the holder side; follower-side expiry is not discussed). No `tasks/` entry.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P3
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs:551-558`: `Utc::now() > renew_time + lease_duration`.
- No observed-time/monotonic tracking on the follower side; resourceVersion CAS (`:540-543` comment) prevents simultaneous writes but not a skewed early takeover.
- client-go's documented observedTime approach avoids this; cheap fix, rare trigger — P3 correct.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3 (unchanged)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs:552-556,842-844`: the chrono→jiff migration replaced `Utc::now() > renew_time + duration` with `lease_expired(renew_time, duration, Timestamp::now())` = `now.duration_since(renew_time) > duration`.
- This only avoids adding to an untrusted timestamp (overflow); it still compares the holder-written `renewTime` with the follower wall clock. No observed-time/monotonic tracking was added.
