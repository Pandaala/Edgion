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
├── 3. leader_election.preflight_check()   # preflight checks (failure returns Err and the process exits)
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
    │   └── cleanup on exit (stop ACME, clear registry)
    └── match exit:
        ├── Shutdown → return
        └── LostLeadership → set_config_sync_server(None) + clear + loop back
```

Non-leader replicas are entirely idle: they start no watchers and consume no K8s API connections or memory.
On loss of leadership, all state is cleared and the loop returns to waiting.

## All-Serve Lifecycle

```
run_all_serve_lifecycle()
└── loop:
    └── run_serving_flow()                # do not wait for leadership; start immediately
        ├── start_event_watchers_all_serve()
        │   ├── Controller (with leader_handle)   # status writes are guarded
        │   ├── Caches watcher (no ACME)          # ACME is moved to leader services
        │   ├── Leadership watcher (bidirectional)
        │   └── Reload watcher
        ├── event loop:
        │   ├── CachesReady → set gRPC server
        │   │   └── if is_leader → start_leader_services()
        │   ├── LeadershipAcquired
        │   │   └── if caches_ready → start_leader_services()
        │   ├── LeadershipLost
        │   │   └── stop_leader_services() (continue serving gRPC!)
        │   └── ControllerExit / ReloadRequested → exit
        └── cleanup: set_config_sync_server(None), clear registry
```

All replicas independently run K8s watchers and the gRPC service; the Leader additionally takes on status writeback and ACME.
For a detailed comparison see [01-ha-mode.md](01-ha-mode.md).

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
    │   └── update the Lease via Server-Side Apply
    └── match result:
        ├── Ok(true)  → store is_leader=true (Release), update Pod label
        ├── Ok(false) → store is_leader=false (Release), update Pod label
        └── Err       → assume lost, store is_leader=false (Release), update Pod label
```

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
Label updates are fire-and-forget; failures do not block the election loop.

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
    ControllerExit(reason),       // KubernetesController exited (normal/410 Gone/error)
    ReloadRequested,              // Admin API triggered a reload
}
```

The main loop is a simple `match event_rx.recv().await`, with no complex `select!`; the event-handling logic is straightforward:

| Event | leader-only handling | all-serve handling |
|-------|-----------------------|--------------------|
| `CachesReady` | Create ConfigSyncServer + register WatchObjs | Same as left; if already leader, additionally start_leader_services |
| `CachesTimeout` | Same as above (degraded) | Same as above |
| `LeadershipLost` | Exit run_main_flow | stop_leader_services, keep serving gRPC |
| `LeadershipAcquired` | Not used | If caches_ready, start_leader_services |
| `ControllerExit` | Exit the loop, trigger retry/rebuild | Same as left |
| `ReloadRequested` | Exit the loop, trigger reload | Same as left |

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
- Effective only in Kubernetes mode; FileSystem mode does not have this option

### Watcher Tasks

`start_event_watchers()` spawns four background tokio tasks:

| Task | Responsibility | Events emitted |
|------|----------------|----------------|
| **Controller** | Run `KubernetesController.run()` (spawn all ResourceControllers) | `ControllerExit(reason)` |
| **Caches** | Wait for all phased processors to register + all sync-to-Gateway kinds to be ready | `CachesReady` / `CachesTimeout` |
| **Leader** | Monitor leadership state changes (poll the `AtomicBool` every 100ms) | `LeadershipLost` / `LeadershipAcquired` |
| **Reload** | Listen for Admin API reload signals (`mpsc::Receiver`) | `ReloadRequested` |

**Leader watcher initial-state broadcast** (all-serve mode):
At startup, the leader watcher checks the current state immediately; if it is already leader it emits a `LeadershipAcquired` immediately rather than waiting for the next 100ms tick.
This ensures that, after a replica restart (when it was already leader), it can resume leader services right away.

The `WatcherHandles` struct holds the `JoinHandle` for all four tasks and uses `abort_and_wait()` to terminate them uniformly during cleanup.

## ConfigSyncServer Publish Timing

The publication of ConfigSyncServer (`set_config_sync_server(Some(...))`) must satisfy strict timing conditions.

### Problem Scenario

In Kubernetes phased initialization, Phase 2 resources (HTTPRoute, GRPCRoute, EdgionTls, EdgionPlugins, etc.)
are registered into `PROCESSOR_REGISTRY` only after Phase 1. If the Caches watcher emits `CachesReady` based purely on
"the registry currently has no not_ready kinds", the ConfigSyncServer would only contain the Phase 1 WatchObj set. A Gateway calling `List(kind)` for Phase 2 kinds would receive `Unknown kind`.

### Correct Rule

1. **Wait for all phased processors to finish registering** (Phase 1 + Phase 2 are all spawned)
2. **Wait for all kinds that sync to Gateway to be marked ready** (init phase complete)
3. **Run `PROCESSOR_REGISTRY.all_watch_objs(no_sync_kinds)` and emit `CachesReady`**

### Troubleshooting Tip

If Gateway logs repeatedly show `Failed to list resources: Unknown kind: HTTPRoute/GRPCRoute/EdgionTls/...`,
but existing traffic still works, prioritize checking whether only a partial set of WatchObjs was published after a Controller restart/reload.

## Retry and Backoff

`run_main_flow` has a built-in exponential-backoff retry mechanism:

| Parameter | Value |
|-----------|-------|
| Backoff formula | `2^min(failures, 6)` seconds (1s → 2s → 4s → 8s → 16s → 32s → 64s) |
| Max consecutive failures | Give up after 10 (process exits) |
| Stable-run reset | Reset the failure counter after 5 minutes of continuous running |

Each abnormal controller exit or 410 Gone-triggered rebuild increments the consecutive-failure count by 1.
If a single iteration runs stably for more than 5 minutes, the count is reset to zero.

## Configuration Reference

```yaml
conf_center:
  type: kubernetes
  gateway_class: edgion
  controller_name: edgion.io/gateway-controller
  ha_mode: leader-only            # or "all-serve"
  leader_election:
    lease_name: edgion-controller-leader
    lease_namespace: edgion-system
    lease_duration_secs: 15
    renew_deadline_secs: 10
    retry_period_secs: 2
```

## Key Files

- `src/core/controller/conf_mgr/conf_center/kubernetes/center.rs` — lifecycle core (`start`, `run_main_flow`, `run_serving_flow`)
- `src/core/controller/conf_mgr/conf_center/kubernetes/leader_election.rs` — leader election implementation
- `src/core/controller/conf_mgr/conf_center/kubernetes/controller.rs` — `KubernetesController` (spawns resource controllers)
- `src/core/controller/conf_mgr/conf_center/kubernetes/config.rs` — `KubernetesConfig`, `HaMode`
- `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs` — `ResourceController<K>`
