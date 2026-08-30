---
name: liveness-probe-and-lifecycle-supervision
description: Use when reviewing K8s-mode controller lifecycle/liveness findings; covers the fixed supervised event loop (startup deadline + JoinSet panic detection + leadership-epoch cancellation), give-up-triggers-shutdown, the /health deadman, and the leader_job_alive gauge, plus five rejected hardening proposals.
status: fixed
finding_id: leader-wedge-recovery-and-worker-death
closed: 2026-07-17
---

# Wedged Leader Never Restarts / Never Fails Over — FIXED (supervised loop + deadline + deadman + gauge)

## Conclusion

The Kubernetes-mode event loops (`run_main_flow`, `run_serving_flow` in
`edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs`) now observe their
watcher tasks through a `JoinSet`-backed `select!`, arm a configurable pre-ready deadline
(`cache_ready_deadline_secs`, default 90 s) that is disabled once caches are ready, and
cancel the caches-ready tail via a leadership-epoch `CancellationToken` so a demoted
replica cannot start leader jobs and report ready with them dead. Exhausting in-process
recovery (`MAX_CONSECUTIVE_FAILURES`) now calls `shutdown_handle.shutdown()` before
returning — in both leader-only and all-serve flows — so the process actually exits and
Kubernetes can restart/reschedule it. A `/health` deadman (`mark_lifecycle_terminated`,
observed via an inner `JoinHandle` so a lifecycle panic is caught too) backstops any
unforeseen terminal state. Each leader-job worker (ACME, conf_sync CA rotation, DNS
resolver) is wrapped by `spawn_observed`, publishing generation-gated
`edgion_controller_leader_job_alive{leader_job}`.

Fix suggestions of the form "recheck `is_leader()` before starting leader jobs",
"`catch_unwind` in the callee", "auto-restart a dead leader-job worker",
"cancel the all-serve caches task from the leadership watcher", or "write gauge=0
unconditionally on every stop" are **not accepted**.

## Core Rationale

**1. `JoinHandle` observation alone cannot detect a hang — it only detects a panic**

A hung task neither completes nor errors, so polling `JoinHandle`s catches self-inflicted
crashes but not a backpressure deadlock (e.g. a bounded-mpsc `send().await` into a full
workqueue inside the caches-ready tail). The design therefore pairs two independent
detectors — `JoinSet` completion/panic observation *and* a loop-level startup deadline —
and neither can be dropped: panics need the former, hangs need the latter. Normal watcher
completion (every watcher sends its event before returning) is proven safe to ignore via
the `biased` `select!` ordering, so the `JoinSet` arm never races a real event out from
under the loop.

**2. "A liveness probe that cannot fail is not a liveness probe"**

Before this fix, `/health` was an unconditional `200 OK` with no state at all — Kubernetes'
liveness probe existed on paper but could never observe the one failure mode it exists to
catch (event loop wedged, lease still held, Admin API still answering, readiness correctly
503 but nothing restarts on readiness failure). The recovery design keeps in-process
self-healing as primary (no pod restart for a recoverable fault) and treats `/health` as a
last-line deadman for states the in-process recovery didn't anticipate — but that backstop
has to be able to flip, or it is decorative.

**3. Give-up-without-shutdown was itself a wedge**

The pre-fix give-up path (exceeding `MAX_CONSECUTIVE_FAILURES`) returned
`MainFlowExit::Shutdown` from a detached lifecycle task with nothing downstream acting on
it: the process stayed alive, the Admin API kept answering, and `LeaderElection::run` kept
renewing the lease from an independent task — control plane dead, leadership held, probes
green. The fix makes both over-limit sites call `shutdown_handle.shutdown()` before
returning, which releases the lease (reusing the separately-fixed
`leader-election-wedge-and-lease P2` release-on-shutdown path) and lets Kubernetes restart
the pod so another replica can take over. All-serve mode previously had no give-up at all
(fixed 1 s/5 s sleep loop forever); it now shares the same failure-counter + backoff +
give-up logic via an `ServingFlowExit` failure-vs-benign split.

## Fix Suggestions Not Accepted

- **Naive `is_leader()` recheck before starting leader jobs** — the flag is 2–10 s stale by
  design (the `within_deadline` lease-retention branch); skipping the start on a stale
  `false` while still unconditionally sending `CachesReady` produces the exact bug this
  fix closes (a replica reports ready with all leader jobs dead). The real fix is the
  leadership-epoch `CancellationToken`: the leadership watcher cancels it *first*, then
  sends `LeadershipLost`, and the caches task re-checks the token immediately before
  `start_leader_jobs` and returns early (neither starting jobs nor sending `CachesReady`)
  if cancelled.
- **`catch_unwind` in the callee** — the panic surface is inside the spawned watcher/leader-
  job tasks, unreachable from a caller-side `catch_unwind`; `JoinHandle`/`JoinSet`
  observation at the task boundary is the correct layer, and it also has to coexist with
  hang detection anyway (rationale 1).
- **Per-leader-job auto-restart on death** — rejected at the design-decision stage (P3
  depth = metric + alert only). A worker panic usually means corrupted state; a blind
  restart risks a panic loop. Recovery is by alert + operator action
  (`/api/v1/reload` or pod restart).
- **Watcher-driven token cancel in all-serve mode** — `LeadershipLost` is routine and non-
  fatal there (the loop stops leader services and continues serving as a follower); a
  `CancellationToken` is not resettable, so wiring the all-serve leadership watcher to
  cancel it would permanently kill the caches task of any replica that briefly held
  leadership during its startup window — a designed-in follower outage. The wiring is
  deliberately different between `start_event_watchers` (leader-only, watcher-cancels) and
  `start_event_watchers_all_serve` (cleanup-only cancel) — do not copy-paste one onto the
  other.
- **Ungated gauge-0 writes on every stop** — ACME/rotation stop is drain-based (dropping
  the sender lets a mid-command worker run until it observes `recv() == None`), so on a
  fast stop→start cycle the OLD worker's observer can fire *after* the NEW generation
  already set the gauge to 1; an ungated 0-write there would wedge a live worker's gauge at
  0 and make the alert page permanently-false. Gauge→0 on orderly stop is gated on a handle
  actually being taken (`guard.take().is_some()`); gauge→0 on unexpected death is written
  only for a still-current generation, with the generation check and the write performed
  under the same lock to close the TOCTOU. `stop_leader_jobs` itself stays observability-
  neutral (it runs unconditionally on cleanup, including paths where nothing ever started)
  to avoid minting a forbidden 0-series for a job that was never spawned (e.g.
  `conf_sync_rotation` on a non-auto_ca leader).

## Re-evaluation Triggers

Re-open this decision only if:

- A post-ready progress-liveness feature is proposed. **Accepted limitation today: post-
  ready stalls stay silent.** The startup deadline branch is disabled once `caches_ready`
  is set, so a steady-state backpressure stall (e.g. a bounded-workqueue send from inside
  a consumer) or a hung-but-not-dead controller task after readiness still wedges silently
  with probes green — P1's scope is the startup wedge only, and a general progress-
  liveness signal for the steady-state phase is a separate feature. Re-open once such a
  feature is proposed.
- Leadership fencing is added. **Accepted limitation today: the `is_leader` flag's
  inherent 2–10 s staleness window** (a lease-design property) is unchanged by this fix —
  a demoted replica can run leader jobs for up to that long before the epoch token catches
  up. A fencing token/epoch check at the data-plane boundary would change the "naive
  `is_leader()` recheck" rejection above.
- The bounded workqueue's per-kind capacity (5000) changes, or the sizing rule of thumb in
  the config docs stops holding — `cache_ready_deadline_secs` is restart-bound (a
  `/api/v1/reload` always re-arms with the boot-time value), so an undersized deadline
  becomes a *permanent* crash loop, not a one-time failure.

## Reference Cases

- Design doc: `docs/superpowers/specs/2026-07-16-leader-wedge-recovery-design.md`
- Implementation plan: `docs/superpowers/plans/2026-07-17-leader-wedge-recovery.md`
- Origin task (deleted on closure): `tasks/todo/leader-wedge-recovery-and-worker-death.md`
  (split from `tasks/todo/leader-election-wedge-and-lease.md`, whose P2 — lease release on
  shutdown — was fixed separately; see
  [leader-lease-release-on-shutdown.md](../concurrency/leader-lease-release-on-shutdown.md))
- Sibling call-site cleanup that this fix builds on:
  [leader-jobs-call-site-consolidation.md](leader-jobs-call-site-consolidation.md)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs` — supervised
  `select!` loop, `spawn_caches_ready_task` epoch-token check, give-up-triggers-shutdown
  sites
- `edgion-controller/src/config/conf_center/kubernetes.rs` — `cache_ready_deadline_secs`
  validation and default
- `edgion-controller/src/api/mod.rs`, `edgion-controller/src/cli/mod.rs` — the `/health`
  deadman flag and its lifecycle-`JoinHandle` wrapper
- `edgion-controller/src/services/supervision.rs`,
  `edgion-controller/src/conf_mgr/sync_runtime/metrics.rs` — `spawn_observed` and the
  `edgion_controller_leader_job_alive` gauge
- `edgion-controller/src/services/{acme/service.rs, conf_sync_ca/rotation.rs,
  dns_resolver/service.rs}` — per-service orderly-vs-unexpected-death classification
