---
name: leader-lease-release-on-shutdown
description: Use when reviewing K8s leader-election shutdown findings; covers the best-effort lease release (client-go ReleaseOnCancel equivalent) added to the graceful-shutdown branch, and why holder-clearing alone or a Drop impl are rejected.
status: fixed
finding_id: leader-election-wedge-and-lease P2
closed: 2026-07-16
---

# Leader Lease Never Released on Shutdown — FIXED (best-effort release in the shutdown branch)

## Conclusion

`LeaderElection::run`'s shutdown branch (`leader_election.rs`) now calls
`release_lease` — a `get` + CAS `replace` that clears **both** `holder_identity`
and `renew_time` — before patching the pod label and returning, bounded by
`RELEASE_LEASE_TIMEOUT_SECS` (3 s). A successor's next `try_acquire_or_renew`
tick sees the lease as expired ("no renew time means expired") and takes over in
≤ `retry_period_secs` (2 s), instead of waiting out
`lease_duration + retry_period` (~17 s) on every graceful shutdown / rolling
upgrade. Re-reports of the form "the lease keeps `holder_identity` after
shutdown / no ReleaseOnCancel / every rolling upgrade pays ~17 s of leaderless
time" are **not accepted**.

Fix suggestions of the form "release in a `Drop` impl", "delete the Lease
object on shutdown", or "make the release blocking/retried until success" are
**not accepted**.

## Core Rationale

**1. Clearing `holder_identity` alone would NOT unblock a successor.**

This implementation's acquire predicate is `is_current_holder || is_expired`
(`try_acquire_or_renew`); there is no vacant-holder check (unlike client-go,
which treats an empty `HolderIdentity` as acquirable). Expiry is derived from
`renew_time + lease_duration_seconds`, and a `None` `renew_time` is treated as
expired. Clearing `renew_time` together with the holder is therefore the
minimal release that works without touching the acquire logic.

**2. Best-effort with a 3 s bound, not guaranteed.**

The kube client's default request timeout is far longer than the post-signal
grace window (the process is only guaranteed alive ~2 s+ after SIGTERM — the
gRPC shutdown delay in `cli/mod.rs` — with a 25 s force-exit cap). An
unresponsive apiserver must not stall shutdown; on timeout/error the code logs
a WARN and falls back to natural lease expiry, which is exactly the
pre-release behavior. Same "best-effort on the exit path" stance as the pod
label patch (see [leader-pod-label-tick-resync.md](leader-pod-label-tick-resync.md)).

**3. The CAS `replace` cannot clobber a successor.**

`release_lease` re-reads the Lease, no-ops if this identity is no longer the
recorded holder, and carries the read `resourceVersion` into `replace()`; a
409 means another writer (a successor acquiring) won the race — the release
goal is already met, so 409 is success.

**Known hazards, verified acceptable (adversarial re-review, 2026-07-16):**

- *Same-identity successor incarnation*: the holder check compares `POD_NAME`,
  so an in-place binary restart in the same pod (the dev hot-replace flow,
  `skills/09-misc/03-dev-deploy-hot-replace.md`) could in theory have the old
  process release the new incarnation's freshly acquired lease. Requires a
  multi-second-delayed GET overlapping a fast successor start, self-heals
  within one tick (`renew_time: None` → the new incarnation re-acquires), and
  client-go's `ReleaseOnCancel` has the identical property. Not reachable for
  Deployments (unique pod names).
- *Truncation by process exit*: the election task is detached; a release still
  in flight when the runtime drops is silently cancelled (the 3 s bound does
  not gate exit). Outcome equals pre-release behavior.
- *Routine handoff overlap*: fast takeover (~2 s) means a departing pod's
  in-flight leader-gated write can now routinely overlap the new leader's
  writes (previously only when drain exceeded ~17 s expiry). `is_leader` is
  flipped false before the release, so new gated checks fail correctly; this
  is the standard, accepted cost of ReleaseOnCancel-style handoff.
- *Follower-shutdown gap*: shutdown with local `is_leader == false` (e.g.
  renew-deadline step-down immediately followed by SIGTERM) skips the release
  while the server may still record this pod as holder. In that state the
  apiserver was already unreachable, so a release would fail too — the gap
  forgoes an improvement but is never a regression.

## Fix Suggestions Not Accepted

- `Drop` impl on `LeaderElection` — release requires async apiserver I/O;
  `Drop` cannot await, and the election task may be aborted rather than
  dropped cleanly. The abort/wedge paths are P1's territory
  (`tasks/todo/leader-wedge-recovery-and-worker-death.md`), not fixable here.
- Delete the Lease object — loses `lease_transitions` history and races the
  404-recreate path in the same loop; clearing spec fields is strictly safer.
- Blocking/retried release until success — the loop is exiting; retrying
  against a broken apiserver delays pod termination for zero benefit over
  natural expiry.
- Back-dating `renew_time` past the lease duration — equivalent outcome to
  clearing it, but leaves a misleading timestamp on the object; `None` states
  "released" unambiguously.

## Re-evaluation Triggers

Re-open this decision only if:

- The acquire predicate gains a vacant-holder check (then clearing
  `renew_time` becomes redundant and the release could preserve it).
- P1's wedge-recovery work introduces leadership-epoch cancellation that
  needs release-on-demotion (not just release-on-shutdown).
- The post-signal grace window shrinks below `RELEASE_LEASE_TIMEOUT_SECS`.

## Reference Cases

- Original finding: `tasks/todo/leader-election-wedge-and-lease.md` P2 (closed 2026-07-16;
  P1/P3 split to `tasks/todo/leader-wedge-recovery-and-worker-death.md`)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs` —
  `release_lease`, `RELEASE_LEASE_TIMEOUT_SECS`, the shutdown branch in `run()`
- [leader-pod-label-tick-resync.md](leader-pod-label-tick-resync.md) — sibling
  decision documenting the best-effort one-shot label patch on the same branch
- client-go `LeaderElector.release()` (`ReleaseOnCancel`) — the upstream
  precedent this mirrors
