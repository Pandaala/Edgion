---
name: kubernetes-ha-mode
description: Kubernetes HA modes — leader-only vs all-serve behavior matrix, architecture diagram, status-write guard, ACME leader-only, leader-switchover reconciliation.
---

# Kubernetes HA Modes

The `ha_mode` configuration option supports two multi-replica modes for different scale and availability requirements.

## Active leader renewal deadline

The Kubernetes election task owns one monotonic deadline measured from the last
acknowledged Lease acquisition or renewal. It remains active while sleeping,
awaiting Lease GET/CAS, or updating the derived Pod label. Expiry withdraws the
local leader flag and stored fence before further IO. A successful response ready
at or after expiry cannot restore that term; reacquisition requires a fresh Lease
read and successful resourceVersion CAS with a new local term.

Transient Lease errors retain authority only within the remaining window; 404,
409 and a competing holder cause immediate withdrawal. Pod-label convergence runs
after Lease processing and consumes only the existing retry interval, so a slow
label patch does not postpone the next renewal. Timed-out patches retry current
state on later ticks. Each attempt reads the Pod and conditionally patches its
UID/resourceVersion plus a fresh internal `edgion.io/leader-label-write-id` marker.
The marker advances the object version even when the desired label already matches,
so a delayed older patch cannot overwrite a newer acknowledged synchronization.
Conflicts retry with a fresh read; GET and PATCH share the existing time bound.
Labels never establish or extend leadership.

Cancelling a request does not roll back a server write or fence already submitted
status writes. The leader-only lifecycle and all-serve per-write guards retain
their separate semantics. See the [renewal and label regression guard](../../../../04-review/concurrency/regression-guards/leader-pod-label-tick-resync.md)
for ARCH-112 and its verification boundaries.

## Mode Overview

| Mode | Name | Description | Use case |
|------|------|-------------|----------|
| `leader-only` | Leader-Only (default) | Only the Leader builds resource watchers/caches and publishes ConfigSyncServer | Small deployments, resource-sensitive |
| `all-serve` | All Replicas Serve | All replicas build watchers/caches and publish ConfigSyncServer; the Leader also runs leader-only services | High availability, gRPC load balancing required |

### Configuration

```yaml
conf_center:
  type: kubernetes
  ha_mode: leader-only    # or "all-serve"
  leader_election:
    lease_name: edgion-controller-leader
    lease_namespace: edgion-system
```

## Architecture Comparison

```
+-------------------------------------------+--------------------------------------------+
|  leader-only (default)                    |  all-serve                                 |
+-------------------------------------------+--------------------------------------------+
|                                           |                                            |
|  Pod A (leader)                           |  Pod A (leader)       Pod B (non-leader)   |
|  +-------------------------+              |  +------------------+ +------------------+ |
|  | K8s watchers          Y |              |  | K8s watchers   Y | | K8s watchers   Y | |
|  | runtime registry      Y |              |  | runtime registry Y| | runtime registry Y| |
|  | ConfigSyncServer     Y |              |  | ConfigSyncSrv  Y | | ConfigSyncSrv  Y | |
|  | gRPC List/Watch      Y |              |  | gRPC List/Watch Y | | gRPC List/Watch Y | |
|  | Status -> K8s API    Y |              |  | Status -> K8s  Y | | Status -> K8s  N | |
|  | leader services      Y |              |  | leader services Y| | leader services N| |
|  +-------------------------+              |  +------------------+ +------------------+ |
|                                           |                                            |
|  Pod B (standby)                          |  Gateway -> any replica                    |
|  +-------------------------+              |  (edgion-controller Service)               |
|  | election + listeners    |              |                                            |
|  | no resource cache/watch |              |                                            |
|  +-------------------------+              |                                            |
|                                           |                                            |
|  Gateway -> leader only                   |                                            |
|  (edgion-controller-leader Service)       |                                            |
+-------------------------------------------+--------------------------------------------+
```

## Behavior Matrix

| Behavior | `leader-only` | `all-serve` |
|----------|---------------|-------------|
| K8s watchers start time | Leader only | All replicas (immediately) |
| gRPC availability | After the Leader is ready | After any replica is ready |
| `set_config_sync_server(Some)` | Leader + CachesReady | CachesReady (any replica) |
| Status writeback to K8s | Leader (`leader_handle` guard) | Leader (`leader_handle` guard) |
| Leader-only services (ACME, conf-sync CA rotation, status reconciliation) | Leader (implicit; main flow starts only on leader) | Leader (explicit `start_leader_services()`) |
| Federation client (`center.enabled`) | Leader | Leader — a follower keeps serving reads but does **not** federate |
| Leadership-loss handling | Exit `run_main_flow`, clear everything, return to `wait_until_leader` | Only `stop_leader_services()`; keep serving gRPC |
| Gateway connection target | `edgion-controller-leader` Service | `edgion-controller` (all-pods) Service |
| gRPC interruption on leader switchover | Yes (cache rebuild ~5–15s) | No (other replicas keep serving) |
| K8s API server load | 1x watcher connection | Nx watcher connections (N = replica count) |
| Memory footprint | Only the Leader uses cache memory | All replicas use cache memory |

## Federation Client Is Leader-Only in Both Modes

The federation session at Center is keyed by `controller_id` (`cluster/name`), which is
identical on every replica; registering a second session for the same `controller_id`
displaces and cancels the first. The federation supervisor's run predicate is therefore
`center.enabled && is_leader` (`fed_sync::supervisor::should_run`), fed by
`CenterLifeCycle::leader_signal()`.

Each actual client start receives the supervisor's current immutable Center
configuration snapshot (ARCH-083), including an address/identity restart or a
promotion after a follower received reloads. The CLI spawn closure consumes that
snapshot rather than retaining the startup configuration. Policy-only reloads
still use the live Authorizer without restarting the connection; this does not
add restart triggers for security, environment or tag fields.

This is the one behavior in `all-serve` where a follower does less than the leader while
still serving gRPC reads normally. The leadership signal used here is deliberately NOT
`leadership_status()`, which stays `NotApplicable` in Kubernetes so the Admin API
not-leader 409 gate remains disabled.

Metrics: `edgion_fed_kill_switch_state` reports only the operator's `center.enabled`
intent, and `edgion_fed_client_running` reports whether the client is actually running.
A follower reporting `kill_switch_state=1, client_running=0` is the healthy steady state.

## Status-Write Leader Guard

All replicas compute status via `ResourceProcessor` (to keep gRPC cache data correct),
but only the Leader writes back to the K8s API. Guard logic:

```rust
let can_write_status = leader_handle.as_ref().is_none_or(|h| h.is_leader());
if status_changed && can_write_status {
    persist_k8s_status::<K>(...).await;
}
```

This guard is applied consistently at three checkpoints:
1. **Init phase** — during `Event::InitApply`
2. **Init-phase edge case** — when `Event::Apply` arrives before `init_done`
3. **Runtime worker** — for work items processed in `spawn_worker`

Kubernetes controllers always receive a `LeaderHandle`; non-Kubernetes centers do not call this K8s status-write path.

### Why Not All Replicas Should Write Status

If all replicas wrote status, an infinite loop would form:

```
Pod-A computes ResolvedRefs=True → writes to K8s
  → Pod-B's cache has not yet synced this Secret → computes ResolvedRefs=False → overwrites K8s
    → watch event bounces to all replicas → reprocess → loop...
```

## ACME Must Be Leader-Only

The ACME HTTP-01 challenge requires single-point certificate request + single-point validation response.
Multiple replicas requesting at the same time would cause challenge conflicts and issuance failures.

- **leader-only mode**: ACME runs only on the leader implicitly (the entire main_flow runs only on the leader)
- **all-serve mode**: `start_acme_service()` is invoked only inside `start_leader_services()`

```rust
start_leader_services():
├── start_acme_service(client)
├── start_conf_sync_rotation(client)
├── start_credential_manager_service(client, runtime, leader)
└── runtime.processor_registry.requeue_all()    // full-status reconciliation

stop_leader_services():
├── stop_acme_service()
├── stop_conf_sync_rotation()
└── stop_credential_manager_service()
```

The Controller DNS resolver is not a leader job: it publishes in-memory slices,
so it runs in every process that runs the resource pipeline (all-serve followers
included), started with the first `controller`-mode Service and stopped by
`ControllerRuntime::reset`.

## Leader-Switchover Reconciliation (requeue_all)

After a new Leader takes over, it calls `runtime.processor_registry.requeue_all()` to trigger a full status recomputation and writeback.

**Why reconciliation is needed**: the previous Leader may have left stale status before losing leadership;
the new Leader's cache may differ from the status currently in the K8s API.
A full requeue ensures all resources' status is consistent with the new Leader's cache.

`requeue_all()` walks every processor in that center's runtime-owned registry,
fetches all keys from each `ServerCache`, and asynchronously enqueues them into the workqueue.
The Workqueue's dedup mechanism prevents duplicate processing.

## server_id and Gateway Reconnect

Each replica has its own `ConfigSyncServer` instance, which generates a different `server_id`.

The Gateway detects `server_id` changes in the gRPC Watch stream:
- **leader-only mode**: after a leader switchover, the new leader creates a new ConfigSyncServer (new server_id);
  upon detecting the change, the Gateway performs a full re-list
- **all-serve mode**: when the Gateway switches its connection to a different replica, it sees a different server_id and performs a full re-list

This is the correct design behavior: it ensures the Gateway is fully consistent with the Controller replica it is currently connected to.

## Gateway Connection Target

| HA mode | Gateway configuration | Description |
|---------|----------------------|-------------|
| leader-only | `server_addrs = ["http://edgion-controller-leader.edgion-system.svc:12151"]` | Connect only to the leader |
| all-serve | `server_addrs = ["http://edgion-controller.edgion-system.svc:12151"]` | Any replica |

## K8s Service Topology

### leader-only mode

| Service | Selector | Purpose |
|---------|----------|---------|
| `edgion-controller` | `app: edgion-controller` | Health checks, metrics |
| `edgion-controller-leader` | `app: edgion-controller` + `edgion.io/leader: "true"` | Gateway gRPC, Admin API |

### all-serve mode

| Service | Selector | Purpose |
|---------|----------|---------|
| `edgion-controller` | `app: edgion-controller` | Gateway gRPC, health checks, metrics |
| `edgion-controller-leader` | `app: edgion-controller` + `edgion.io/leader: "true"` | Leader-only Admin API writes, ACME callback |

## /ready Endpoint Semantics

`is_ready()` checks `runtime.processor_registry.is_all_ready() && config_sync_server.is_some()`:

- **leader-only**: non-leaders return not ready because they have no published ConfigSyncServer; process-level listeners still run
- **all-serve**: every ready replica returns ready and can receive Gateway traffic

## Migration Paths

### Single replica → HA (leader-only)

1. Deploy `leader-service.yaml` (with the `edgion.io/leader: "true"` selector) and RBAC (Pod patch permissions)
2. Point Gateway `server_addrs` at `edgion-controller-leader`
3. Set `replicas: 2`

### leader-only → all-serve

1. Change `ha_mode = "all-serve"`
2. Point Gateway `server_addrs` at `edgion-controller` (all-pods Service)
3. Rolling restart

### Rollback all-serve → leader-only

1. Change `ha_mode = "leader-only"`
2. Point Gateway `server_addrs` at `edgion-controller-leader`
3. Rolling restart

### RBAC Requirements

Pod-label patch requires extra permissions:

```yaml
rules:
- apiGroups: [""]
  resources: ["pods"]
  verbs: ["get", "patch"]
```

## Key Files

- `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs` — `run_leader_only_lifecycle`, `run_all_serve_lifecycle`, `run_main_flow`, `run_serving_flow`
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/leader_election.rs` — `LeaderElection`, `LeaderHandle`
- `edgion-controller/src/fed_sync/supervisor.rs` — `should_run`, `run_supervisor` (federation leadership gate)
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/config.rs` — `HaMode` enum
