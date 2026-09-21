---
name: leader-status-write-per-write-guard-vs-lifecycle-gate
description: Use when reviewing findings that the per-write `is_leader()` AtomicBool status-write guard should become a client-go-style lifecycle gate (CancellationToken quiesce that drains/cancels in-flight workers on leadership loss); explains why the per-write guard is the correct minimal mechanism for AllServe mode.
status: accepted-tradeoff
finding_id: controller-05
closed: 2026-06-14
knowledge_type: intentional-design
---

# Per-Write `is_leader()` Status Guard Is Not a Bug vs client-go Lifecycle Gate — ACCEPTED-TRADEOFF

## Conclusion

The worker loop's per-write `leader_handle.is_leader()` check before `persist_k8s_status`
(`resource_controller.rs`) is the **accepted minimal leadership gate** for Edgion's
`AllServe` HA mode, not a defective substitute for client-go's `OnStoppedLeading` /
context-cancellation lifecycle gate. It does not fence an already-started write at the
API server or prove that only the current leader can write. The shared status persistence
path now rejects stale raw resourceVersions with 409; this protects against intervening
object updates, not leadership changes themselves. A demoted writer with a current version
can still pass that condition. Later reconciliation can refresh status, without a time bound.

Historical correction: this page previously asserted that the original unconditional
Merge Patch already supplied resourceVersion conflict protection. That assertion was
false. The separate [snapshot concurrency correction](../../status/regression-guards/kubernetes-status-snapshot-concurrency.md)
adds the actual precondition; it does not strengthen the leadership gate into fencing.

Fix suggestions of the form "introduce a `CancellationToken` created on leadership acquire
and cancelled on leadership loss, thread it into every worker, and have
`stop_leader_services()` drain in-flight workers with a timeout (match client-go
`OnStoppedLeading`)" are **not accepted**.

## Core Rationale

**1. In AllServe, a demoted leader continuing to commit `ServerCache` / fire `on_change` is the design contract, not a leak**

`AllServe` mode (`center.rs::run_serving_flow`) has *every* replica run watchers + workers
and push config to its own connected Gateways — each replica is an independent config-sync
source. Leadership gates only two things: (a) K8s status writes (the per-write
`is_leader()` check), and (b) leader-only services / ACME (`start/stop_leader_services`).
A "just-demoted" leader is now simply a normal non-leader, behaving identically to every
other non-leader: it keeps committing to its own `ServerCache` and firing local `on_change`
requeues. There is no anomalous "spurious side effect" — that is steady-state AllServe
behavior on all replicas.

**2. client-go's `OnStoppedLeading` is the leader-only lifecycle model, which Edgion's `leader-only` mode already matches**

client-go leader election = leader-only model (only the leader runs the reconcile loop;
loss cancels the context). Edgion's `leader-only` HA mode already matches this: non-leader
replicas block in `wait_until_leader_with_shutdown` (`center.rs:538`) and never run
workers, so there is no concurrent reconcile to drain. Imposing the cancel-on-loss model
on `AllServe` is **wrong**: cancelling/draining in-flight workers on leadership loss would
blackhole that replica's config supply to its downstream Gateways — the opposite of what
AllServe requires.

**3. The per-write leadership check is the accepted minimal gate**

Only status writes must be single-writer; config push is multi-writer *by design* in
AllServe. Gating exactly the status write (and nothing else) through `LeaderHandle::is_leader`
reads the shared watch-channel boolean; the former AtomicBool implementation is gone.
A leadership transition can still leave an in-flight write. Snapshot
version comparison rejects it only if another update has already changed the object;
the accepted lifecycle gate and snapshot concurrency protection have distinct scopes.

**4. The `leader_handle = None` injection risk is not reachable in multi-replica K8s**

`Option<LeaderHandle>` is `None` only for FileSystem / single-replica modes (no election).
Both K8s lifecycles unconditionally wire it via `with_leader_handle(leader_handle.clone())`
at two adjacent sites (`center.rs:465` leader-only, `center.rs:825` all-serve), so a
multi-replica deployment cannot accidentally get `None` without editing the wiring layer.
A defensive warn/assert would need to thread HA-mode awareness into the worker for a
statically-impossible state.

## Fix Suggestions Not Accepted

- `CancellationToken` + drain-with-timeout in `stop_leader_services()` — large cross-module
  change (token threaded into every worker spawn point); in AllServe it would cancel
  workers that must keep serving config, blackholing that replica's downstream Gateways.
- Lifecycle-gate the reconcile loop (start on acquire / stop on loss) like client-go — that
  is the leader-only model, already provided by Edgion's `leader-only` mode; it contradicts
  the AllServe "all replicas serve" contract.
- Defensive warn/assert when `leader_handle == None` in a multi-replica deployment —
  the state is statically unreachable (wired at two adjacent sites); the check would need
  HA-mode plumbing for no real protection.
- Add a "leadership transition: workers in flight = N" log/metric — the in-flight workers
  are the intended AllServe behavior, not an incident signal.

## Re-evaluation Triggers

Re-open this decision only if:

- A new HA mode is added that requires non-leader replicas to stop serving config on
  leadership loss (i.e. an actual leader-only-with-shared-watchers model).
- Status writes gain a non-idempotent side effect, require server-side leader fencing,
  or require bounded convergence after a dropped write.
- AllServe status writes move off Kubernetes optimistic concurrency (e.g. a backend
  without resourceVersion conflict semantics).

## Reference Cases

- Original finding: `tasks/review-envoy/controller-05-leader-election-atomicbool-poll.md`
  (closed 2026-06-14, severity downgraded high → medium during verification)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs:530` —
  the per-write `is_leader()` status guard
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs` —
  `run_leader_only_lifecycle` (line 525, `wait_until_leader`),
  `run_serving_flow` (line 703, AllServe), `stop_leader_services` (line 907, ACME-only)
- Sibling decisions: [leader-handle-polling.md](leader-handle-polling.md) (watch::channel
  vs polling — different finding), [leader-pod-label-tick-resync.md](../regression-guards/leader-pod-label-tick-resync.md)
- `skills/01-architecture/01-controller/03-config-center/02-kubernetes/01-ha-mode.md` —
  leader-only vs all-serve HA modes
