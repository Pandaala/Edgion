---
name: kubernetes-lifecycle
description: KubernetesCenter startup flow and leader election — Lease-based election, event-driven main loop, watcher task management, ConfigSyncServer publish timing.
---

# Kubernetes ConfigCenter — Lifecycle

## Startup Flow

`KubernetesCenter::start()` is the entry point in Kubernetes mode; it dispatches to different lifecycles depending on `ha_mode`:

```
start(shutdown_handle)
├── 1. Client::try_default()              # create K8s client (in-cluster ServiceAccount or kubeconfig)
├── 2. LeaderElection::new(client, config) # initialize leader election
├── 3. leader_election.preflight_check()   # preflight checks (failure returns Err; readiness stays false)
│      ├── ensure_lease_exists()           #   verify the Lease object is readable/creatable
│      └── ensure_pod_label_patch_ready()  #   verify Pod label is patchable (RBAC permission check)
├── 4. tokio::spawn(le.run())             # only start the background election loop after preflight passes
└── 5. match ha_mode:
    ├── LeaderOnly → run_leader_only_lifecycle(client, leader_handle, shutdown_handle)
    └── AllServe   → run_all_serve_lifecycle(client, leader_handle, shutdown_handle)
```

## Leader-Only Lifecycle (default)

```
run_leader_only_lifecycle()
└── loop:
    ├── wait_until_leader()               # block until this replica becomes Leader
    ├── run_main_flow()                   # run all watchers + gRPC
    │   ├── start_event_watchers()
    │   ├── event loop (CachesReady → set gRPC server)
    │   └── cleanup on exit (stop leader services, reset ControllerRuntime)
    └── match exit:
        ├── Shutdown → return
        └── LostLeadership → unpublish ConfigSyncServer + reset runtime + wait again
```

Non-leader replicas do not start resource watchers, resource caches, or ConfigSyncServer. They still run
the election loop and the process-level Admin, probe, metrics, and gRPC listeners. On loss of leadership,
the leader-only resource state is reset and the lifecycle returns to waiting.

## All-Serve Lifecycle

```
run_all_serve_lifecycle()
└── loop:
    └── run_serving_flow()                # do not wait for leadership; start immediately
        ├── start_event_watchers_all_serve()
        │   ├── Controller (with leader_handle)   # status writes are guarded
        │   ├── Caches watcher (no ACME)          # ACME is moved to leader services
        │   └── Leadership watcher (bidirectional)
        ├── event loop:
        │   ├── CachesReady → set gRPC server
        │   │   └── if is_leader → start_leader_services()
        │   ├── LeadershipAcquired
        │   │   └── if caches_ready → start_leader_services()
        │   ├── LeadershipLost
        │   │   └── stop_leader_services() (continue serving gRPC!)
        │   └── ControllerExit → exit
        └── cleanup: unpublish ConfigSyncServer, stop leader services, reset runtime
```

All replicas independently run K8s watchers and can publish a ConfigSyncServer. The Leader additionally
writes status, runs ACME, rotates the conf-sync CA, runs the DNS resolver, and performs a full requeue on acquisition.
For a detailed comparison see [01-ha-mode.md](01-ha-mode.md).

## Source snapshots and initial readiness

Each resource kind still has one processor, one workqueue, and one worker.
`watch_source.rs::SourceState` owns a native kube Writer/Store per fixed API
scope: all namespaces, one configured namespace, or a cluster-scoped kind.
Multiple configured namespaces are deduplicated and each gets an independent
source. `KubeRawStore` dispatches reads to those stores; it does not copy objects
into another permanent cache. The processor and worker share that raw reader.

`Init` resets only the source's native staging buffer. `InitApply` stages data
without invoking handlers. A valid `InitDone` atomically replaces that source's
committed snapshot and makes `ever_committed` permanently true for this run.
Duplicate completion is ignored before it reaches Writer. Interrupted LISTs
retain committed data; malformed identities/transitions and source EOF fail the
resource controller. Watcher backoff belongs to each source, before stream merge.

Once every source has committed once, source polling pauses. The complete
committed union is replayed serially through the existing initialization hooks,
completion runs once, and one worker starts. A source already doing another
LIST contributes its previous committed snapshot to this barrier. Incremental
events received while other sources initialize update raw state without filling
the bounded queue before its worker exists.

Runtime relists commit first, then enqueue all old and current source keys via
`on_event_key`. Missing objects reconcile as deletes; unchanged objects can
recover a failed status write. The worker handles eligibility loss by deleting
the previous processed object while retaining the replacement raw object.
Store locks never span queue awaits. Cancellation during partial key delivery
abandons the entire epoch; it never resumes that partially delivered queue.

Namespace metadata remains a separate source with no business workqueue. Its
first complete snapshot gates Gateway initialization. Changes during Gateway
initialization are retained for one Selector Gateway refresh after Gateway is
ready; later Apply/Delete/relist changes use the same refresh path. Terminating
Namespaces are absent from both incremental and replacement views. Publication
must also wait for phased startup to finish when Gateway/dependent kinds are
disabled; a ready subset of registered processors is insufficient.

A runtime source relist does not reset ServerCache, repeat initialization, change
the worker, unpublish config-sync, or mark the whole Controller unready. Raw
snapshot replacement is atomic per source, while processed state converges
through the queue. There is no cross-kind or raw-to-processed atomic transaction.
Inline status IO and its existing UID/resourceVersion race are unchanged.

## Leader Election (Lease-based)

Distributed leader election is implemented using a Kubernetes Lease object:

```
LeaderElection
├── client: kube::Client
├── config: LeaderElectionConfig
│   ├── lease_name: "edgion-controller-leader"
│   ├── lease_namespace: "edgion-system"
│   ├── pod_namespace: $POD_NAMESPACE (Downward API)
│   ├── identity: $POD_NAME (Downward API)
│   ├── lease_duration_secs: 15
│   ├── renew_deadline_secs: 10
│   └── retry_period_secs: 2
└── is_leader: Arc<AtomicBool>
```

### Election Loop

```
run():                                    # tokio::spawn background task
└── loop:
    ├── sleep(renew/retry interval)
    ├── try_acquire_or_renew()
    │   ├── read the current Lease
    │   ├── check holder_identity and expiration
    │   └── replace the Lease with its observed resourceVersion (OCC)
    └── match result:
        ├── Ok(true)  → record a successful acquire/renew and converge the Pod label
        ├── Ok(false) → clean step-down (another holder or an OCC race)
        ├── Err(404)  → step down immediately, recreate the Lease, retry
        └── other Err → retain leadership inside renew_deadline; step down after it expires
```

Lease updates use `GET` followed by `replace` with the observed `resourceVersion`, so concurrent contenders
are resolved through Kubernetes optimistic concurrency control rather than Server-Side Apply.

On graceful shutdown, a leader best-effort releases the lease (`release_lease`: clears `holder_identity`
and `renew_time` under the same OCC, bounded by 3 s) so a successor acquires on its next tick (~2 s)
instead of waiting out the ~17 s lease expiry; on any failure the release falls back to natural expiry.
See `skills/04-review/concurrency/regression-guards/leader-lease-release-on-shutdown.md` for the decision record.

**Memory ordering note**: writes to `is_leader` use `Ordering::Release`, reads use `Ordering::Acquire`,
guaranteeing cross-thread happens-before semantics so that the status-write guard remains correct on weakly-ordered architectures (e.g. ARM/M1).

### Pod Leader Label

The Pod label is updated whenever leadership state changes:

```yaml
metadata:
  labels:
    edgion.io/leader: "true"   # or "false"
```

The `edgion-controller-leader` Service uses this selector to route only to the leader Pod.
Label-update failures do not block the election loop, but leave the label marked unsynchronized so every
subsequent election tick retries the patch until it converges.

### LeaderHandle

A lightweight cloneable handle that internally holds an `Arc<AtomicBool>`:

```rust
pub struct LeaderHandle {
    is_leader: Arc<AtomicBool>,
}

impl LeaderHandle {
    pub fn is_leader(&self) -> bool;                          // Acquire load
    pub async fn wait_until_leader(&self);                    // block waiting
    pub async fn wait_until_leader_with_shutdown(&self, ...) -> bool;
}
```

Passed to `ResourceController` for the leader guard in `persist_k8s_status()`.

## Event-Driven Main Flow

`run_main_flow()` uses an event-driven architecture and receives lifecycle events through `mpsc::channel<LifecycleEvent>`:

```rust
enum LifecycleEvent {
    CachesReady,                  // all phased processors are registered + all sync kinds are ready
    CachesTimeout,                // not ready before timeout
    LeadershipLost,               // lost leadership
    LeadershipAcquired,           // acquired leadership (all-serve mode only)
    ControllerExit(reason),       // KubernetesController exited (shutdown/terminal failure)
}
```

The loop drives a shared `next_loop_turn()` helper (a three-way `select!`, not a bare `recv()` — see
[Supervision & recovery](#supervision--recovery) for the mechanics); the event-handling logic per
`LifecycleEvent` is straightforward:

| Event | leader-only handling | all-serve handling |
|-------|-----------------------|--------------------|
| `CachesReady` | Create ConfigSyncServer + register WatchObjs | Same as left; if already leader, additionally start_leader_services |
| `CachesTimeout` | Treat as controller failure and rebuild | Treat as controller failure and rebuild |
| `LeadershipLost` | Exit run_main_flow | stop_leader_services, keep serving gRPC |
| `LeadershipAcquired` | Not used | If caches_ready, start_leader_services |
| `ControllerExit` | Exit the loop, trigger retry/rebuild | Same as left |

## Watcher Tasks

### Gateway API CRD Preflight

`start_event_watchers()` and `start_event_watchers_all_serve()` perform a CRD preflight check (`preflight_check_gateway_api_crds()`) after endpoint-mode resolution and before creating the Controller.

Preflight logic (`version_detection.rs`):
- For each Gateway API resource (GatewayClass, Gateway, HTTPRoute, GRPCRoute, TLSRoute, ReferenceGrant, TCPRoute, UDPRoute, BackendTLSPolicy), probe one by one with `Api::<DynamicObject>::list(limit=1)`
- Resources listed in `no_watch_kinds` are skipped
- Any non-skipped CRD that is missing → collected and reported in one error; the controller fails to start
- The error message includes the list of missing resources, the CRD install command, and a hint about the `no_watch_kinds` configuration

`no_watch_kinds` configuration (`KubernetesConfig`):
- The values are PascalCase kind names, e.g. `["TCPRoute", "UDPRoute", "BackendTLSPolicy"]`
- Listed resources: not probed, no watcher spawned, excluded from `all_processor_kinds()`
- EndpointSlice obeys the same registration and watcher selection as other provider
  kinds. Disabling every processor kind still leaves the separate Namespace
  first-snapshot gate in place (`ARCH-111`).
- Effective only in Kubernetes mode; FileSystem mode does not have this option

### Watcher Tasks

`start_event_watchers()` spawns four background tokio tasks:

| Task | Responsibility | Events emitted |
|------|----------------|----------------|
| **Controller** | Run `KubernetesController.run()` (spawn all ResourceControllers) | `ControllerExit(reason)` |
| **Caches** | Wait for all phased processors to register + all sync-to-Gateway kinds to be ready | `CachesReady` / `CachesTimeout` |
| **Leader** | Monitor leadership state changes (poll the `AtomicBool` every 100ms) | `LeadershipLost` / `LeadershipAcquired` |

**Leader watcher initial-state broadcast** (all-serve mode):
At startup, the leader watcher checks the current state immediately; if it is already leader it emits a `LeadershipAcquired` immediately rather than waiting for the next 100ms tick.
This ensures that, after a replica restart (when it was already leader), it can resume leader services right away.

The `WatcherSet` struct holds all four tasks in a `JoinSet`, the iteration's
resource-epoch `CancellationToken`, and a `TaskTracker` for resource descendants.
Cleanup first withdraws ConfigSyncServer, then cooperatively cancels and joins
top-level tasks. A timeout aborts and joins the remaining wrappers. Only after
these spawners end does the center close and await the tracker, before runtime
reset or a new epoch. See
[Supervision & recovery](#supervision--recovery) for how the set is also used to detect a hung or
panicked watcher while the loop is running, not just to clean up after it exits.

## ConfigSyncServer Publish Timing

The publication of ConfigSyncServer (`set_config_sync_server(Some(...))`) must satisfy strict timing conditions.

### Problem Scenario

In Kubernetes phased initialization, Phase 2 resources (HTTPRoute, GRPCRoute, EdgionTls, EdgionPlugins, etc.)
are registered into the runtime-owned processor registry only after Phase 1. If the Caches watcher emits `CachesReady` based purely on
"the registry currently has no not_ready kinds", the ConfigSyncServer would only contain the Phase 1 WatchObj set. A Gateway calling `List(kind)` for Phase 2 kinds would receive `Unknown kind`.

### Correct Rule

1. **Wait for phased startup to finish**, including the first Namespace snapshot, even when later resource kinds are disabled
2. **Wait for all kinds that sync to Gateway to be marked ready** (init phase complete)
3. **Run `runtime.processor_registry.all_watch_objs(no_sync_kinds)` and emit `CachesReady`**

### Troubleshooting Tip

If Gateway logs repeatedly show `Failed to list resources: Unknown kind: HTTPRoute/GRPCRoute/EdgionTls/...`,
but existing traffic still works, prioritize checking whether only a partial set of WatchObjs was published after a Controller restart or resource-epoch replacement.

## Retry and Backoff

`run_main_flow` has a built-in exponential-backoff retry mechanism:

| Parameter | Value |
|-----------|-------|
| Backoff formula | `2^min(failures, 6)` seconds (1s → 2s → 4s → 8s → 16s → 32s → 64s) |
| Max consecutive failures | Give up after 10 lifecycle failures |
| Stable-run reset | Reset the failure counter after 5 minutes of continuous running |

Each abnormal controller exit increments the consecutive-failure count by 1.
Recoverable source relists, including recovery after 410 Gone, do not exit the
Controller or consume the lifecycle failure budget.
If a single iteration runs stably for more than 5 minutes, the count is reset to zero.
`run_all_serve_lifecycle` uses the identical backoff formula, failure cap, and stable-run reset — see
[Give-up triggers graceful shutdown](#give-up-triggers-graceful-shutdown-both-flows) below for how the two
flows now share the same give-up behavior (both call `shutdown_handle.shutdown()` on give-up, so the
process actually exits instead of idling with readiness stuck false).

## Supervision & Recovery

Origin: `docs/superpowers/specs/2026-07-16-leader-wedge-recovery-design.md`. Before this design, a hang
anywhere in the post-readiness-poll tail (`register_all` → revalidations → `start_leader_jobs` → the
`CachesReady` send) or a panic in a watcher task left the event loop `Pending` forever — leadership held,
`/health` an unconditional `200`, no automatic recovery. The mechanisms below close that gap.

### Supervised event loop

Both `run_main_flow` and `run_serving_flow` replaced the bare `event_rx.recv().await` with a shared
`next_loop_turn()` helper — a `biased` three-way `select!` over the event channel, the `WatcherSet`'s
`JoinSet`, and a pre-ready deadline:

```rust
tokio::select! {
    biased;   // drain queued events before join results, deterministically
    ev = event_rx.recv() => /* Some(event) -> LoopTurn::Event, None -> ChannelClosed */,
    Some(res) = watchers.set.join_next_with_id(), if !watchers.set.is_empty() =>
        /* Ok -> WatcherFinished(name), Err(join_err) -> WatcherPanicked(name) */,
    _ = deadline, if deadline_armed => LoopTurn::DeadlineExpired,
}
```

- **`biased` matters**: every watcher task sends its event before returning, so even if a `WatcherFinished`
  turn wins a race against the event that task just sent, the event is still queued and gets delivered on
  the very next turn — normal completion never loses an event.
- **Hang-vs-panic detector split**: `JoinHandle`/`JoinSet` observation alone cannot detect a *hang* (a hung
  task neither completes nor errors), and a bare deadline alone cannot attribute a *panic* to the task that
  caused it. The two detectors are complementary and both are required: `WatcherPanicked(name)` (an
  `Err(JoinError)` — in-loop this can only be a panic, since our own aborts happen only in post-break
  cleanup) and `DeadlineExpired` both map conservatively to `ControllerExitReason::AllControllersStopped`,
  funneling into the same failure-budget/backoff/give-up path documented above instead of introducing a
  parallel recovery mechanism.
- A closed event channel (`LoopTurn::ChannelClosed`, all senders dropped) is treated the same way.

### `cache_ready_deadline_secs`

The pre-ready deadline is armed for the whole watcher-set lifetime and disarmed once `CachesReady` is
observed (`deadline_armed = !caches_ready`). Its duration is `cache_ready_deadline_secs` on
`KubernetesConfig` (default **90 s** = the internal 30 s readiness-poll budget + a 60 s tail allowance for
`register_all` + revalidations + leader-job start).

- **Validation**: rejected at startup unless `> 30` (the internal readiness-poll budget) — a deadline that
  cannot even cover the poll is a designed-in restart loop. See
  `KubernetesConfig::validate()` in `edgion-controller/src/config/conf_center/kubernetes.rs`.
- **Restart-bound**: the value is read from `self.config`, captured once at
  `KubernetesCenter` construction. Each resource-epoch initialization arms a fresh deadline
  with the **boot-time** value. Changing this field requires a process restart.
  See [`skills/02-features/02-config/00-controller-config.md`](../../../../02-features/02-config/00-controller-config.md)
  for the operator-facing field reference.
- **Sizing rule of thumb**: raise the deadline when a single resource kind's count approaches the per-kind
  workqueue capacity (5000) — the revalidation tail drains that queue at apiserver status-write rates, and
  because every deadline restart runs `runtime.reset()` and replays from scratch, a too-small deadline is a
  *permanent* crash loop (`STABLE_RUN_DURATION` = 300 s is never reached).
- **Worst-case pre-ready wait before a restart fires is ≈ 2× the configured value**: the deadline budget
  also wraps `start_event_watchers`/`start_event_watchers_all_serve` itself via `tokio::time::timeout`
  (guarding a hung `preflight_check_gateway_api_crds`), and if that returns `Ok` the loop's own deadline then
  arms fresh for the tail — so a hang straddling the boundary can consume close to two full budgets before
  the loop gives up.

### Resource-epoch cancellation and descendant ownership

A larger startup deadline helps finite slow processing; it cannot resolve a
capacity deadlock caused by handler-generated requeues before a worker exists.
The source-bootstrap path avoids filling ingress, but does not change the
existing delayed-queue capacity boundary. See the source-snapshot verification
record for the synthetic saturation/cancellation test and its limits.

Each watcher-set generation (one per outer-loop iteration) gets its own `CancellationToken`
(`WatcherSet::new(CancellationToken::new())`); it covers the caches task,
resource controllers, Namespace watcher, and workers.

Resource controllers and Namespace tasks are tracked before becoming runnable.
Each controller owns its child JoinSet, and each resource controller owns its
worker through an abort-on-drop handle. Phase waits supervise individual child
completion, and source polling/key delivery supervises worker termination.
Tracked parents wait for their own children, never the whole tracker (which
would include themselves). The untracked center waits for all tracked descendants.
Closing a tracker alone does not prohibit late registration: joining all root
spawners first and keeping parent membership during child registration prevents
an empty-tracker race.

Native source owners clear staging and committed stores on teardown, including
an abort Drop fallback, because diagnostic readers can outlive their driver.
Graceful teardown drains the worker before clearing raw stores; forced teardown
must finish descendant cleanup before runtime reuse. Aborting a handle is not
proof that its task has finished. A task that never yields has no bounded join
guarantee; config-sync stays unpublished and runtime reset must not proceed.
This tracks resource-state mutators, not every internal Tokio helper such as
the existing workqueue delay scheduler.

- **Leader-only** (`start_event_watchers`): the leadership watcher cancels the token **first**, then sends
  `LeadershipLost` — so the caches task's tail is stopped even while the event loop is still busy handling
  other turns. `run_caches_ready`'s tail races `select!` against `cancel.cancelled()`, and re-checks
  `cancel.is_cancelled()` as a synchronous, no-await checkpoint immediately before `start_leader_jobs`: a
  cancelled tail neither starts leader jobs nor sends `CachesReady` — it just returns. This is what prevents
  a replica from reporting ready with all leader jobs dead.
- **All-serve** (`start_event_watchers_all_serve`): **cleanup-only** cancellation. The bidirectional
  leadership watcher there does **not** touch the token — `LeadershipLost` is a routine, non-fatal event in
  this mode (the loop just stops leader services and keeps serving gRPC). A `CancellationToken` is not
  resettable, so wiring the watcher to cancel it would permanently kill the caches task of a replica that
  briefly held leadership during its own startup window. The token is cancelled only by loop cleanup
  (`abort_and_wait`). **The wiring is deliberately different between the two `start_event_watchers*`
  variants — do not copy-paste one into the other.**
- Residual: the `is_leader` flag's inherent 2–10 s staleness window (see Known limitations below).
  `leader_election.rs` itself is unmodified.

### Give-up triggers graceful shutdown (both flows)

When the failure budget's `on_failure()` returns `FailureAction::GiveUp` (the failure count exceeds
`MAX_CONSECUTIVE_FAILURES` = 10 — i.e. on the 11th consecutive failure with no 5-minute stable run in
between), the lifecycle calls
`shutdown_handle.shutdown()` **before** returning — releasing the lease (best-effort, see below) and letting
the process exit so Kubernetes restarts the pod and another replica can take over. This is symmetric across
both HA modes:

- **`run_main_flow`** (leader-only): both the watcher-start-failure path and the
  `ControllerExitReason::AllControllersStopped` path call `shutdown_handle.shutdown()` on give-up.
- **`run_all_serve_lifecycle`**: gained the same failure counter + backoff + give-up, replacing the previous
  fixed 1 s / 5 s sleep-and-retry-forever behavior. `ServingFlowExit` distinguishes real failures
  (`CachesTimeout` / `AllControllersStopped` / watcher panic / deadline / channel closed — counted toward
  give-up) from **benign** leadership transitions (reset the counter and do
  **not** count toward give-up), mirroring the leader-only classification.

Without this, all-serve mode's previous behavior was to retry forever on persistent failure (~90 s cycle by
default), never triggering failover or paging anyone.

### `/health` deadman

`cli/mod.rs` step 5 spawns the control-plane lifecycle (`ConfMgr.start_with_shutdown()`) inside an *inner*
task and observes it as a `JoinHandle` — necessary because a panic in the lifecycle body itself (not a
watcher task, but the loop/cleanup code around it) unwinds past any flag-setting code written linearly after
the `.await`, and tokio silently swallows a panic at a task boundary otherwise.

- A process-global, sticky `static AtomicBool` (`LIFECYCLE_TERMINATED` in `edgion-controller/src/api/mod.rs`,
  set via `mark_lifecycle_terminated()`) is marked when the lifecycle ends **for any reason other than an
  orderly shutdown** — a foreseen `Err` return, an `Ok(())` early return, or a lifecycle **panic**
  (`Err(JoinError)`) are all treated identically. `health_check()` returns a plain `503` (JSON error body,
  no extra headers — the `Retry-After: 30` header belongs to a different handler, `require_ready_middleware`
  on admin write routes, which explicitly does not gate `/health`) while the flag is set, `200` otherwise.
  A normal, requested shutdown checks
  `shutdown_handle.is_shutdown()` first and never trips the flag; the give-up path above triggers shutdown
  before returning, so it does not trip it either.
- **On an abnormal end, the wrapper also triggers shutdown itself** — mark the flag first (so `/health` is
  immediately correct), then call `shutdown_handle.shutdown()`, releasing the lease within seconds instead
  of waiting out kubelet's liveness-failure threshold (~30 s+). The `/health` 503 remains the backstop for
  the residual case where the shutdown itself wedges.
- **Deliberate behavior change**: K8s-mode `start()` can return `Err` early — without ever calling
  `shutdown()` — on `KubernetesStorage::new`, `Client::try_default`, a missing `POD_NAME`, or a
  `preflight_check()` (RBAC) failure. Before this design such a pod idled forever with a live Admin API and
  green probes; now the deadman marks + triggers shutdown on that path too, so the pod reaches
  `CrashLoopBackOff` via a fast clean-exit restart loop instead of hiding the misconfiguration. Non-K8s conf
  centers are unaffected: their `start()` only returns after the shutdown signal, so the flag cannot
  false-positive there.
- **Lease release on exit is best-effort, not guaranteed**: `release_lease` is bounded to 3 s (see
  [Leader Election](#leader-election-lease-based) above); on any failure or timeout it falls back to natural
  lease expiry (~`lease_duration_secs`). Do not treat "give-up shuts down" as "failover is instant" — it is
  instant only when the release succeeds within the bound.

### Known limitations (accepted)

- **Post-ready stalls stay silent.** The deadline branch is disabled once `caches_ready` is set
  (`deadline_armed = !caches_ready`), so a steady-state backpressure stall after readiness (e.g. a
  workqueue-full requeue loop) or a hung-but-not-dead controller task is not caught by this mechanism. This
  design's scope is the *startup* wedge; a general post-ready progress-liveness signal is a separate,
  unbuilt feature.
- **Leadership fencing.** The `is_leader` flag's 2–10 s staleness window (a lease-design property, not a bug)
  remains: a demoted replica can keep running leader jobs for up to that long. Out of scope for this design.

## Configuration Reference

```yaml
conf_center:
  type: kubernetes
  controller_name: edgion.io/gateway-controller
  ha_mode: leader-only            # or "all-serve"
  cache_ready_deadline_secs: 90   # default; restart-bound, see "Supervision & Recovery" above
  leader_election:
    lease_name: edgion-controller-leader
    lease_namespace: edgion-system
    lease_duration_secs: 15
    renew_deadline_secs: 10
    retry_period_secs: 2
```

## Key Files

- `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs` — lifecycle core (`start`, `run_main_flow`, `run_serving_flow`)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs` — leader election implementation
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs` — `KubernetesController` (spawns resource controllers)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/config.rs` — `KubernetesConfig`, `HaMode`
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs` — `ResourceController<K>`
