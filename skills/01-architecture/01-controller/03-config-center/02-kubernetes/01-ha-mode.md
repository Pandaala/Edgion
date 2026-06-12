---
name: kubernetes-ha-mode
description: Kubernetes HA modes — leader-only vs all-serve behavior matrix, architecture diagram, status-write guard, ACME leader-only, leader-switchover reconciliation.
---

# Kubernetes HA Modes

The `ha_mode` configuration option supports two multi-replica modes for different scale and availability requirements.

## Mode Overview

| Mode | Name | Description | Use case |
|------|------|-------------|----------|
| `leader-only` | Leader-Only (default) | Only the Leader runs watchers + gRPC; non-leader replicas are entirely idle | Small deployments, resource-sensitive |
| `all-serve` | All Replicas Serve | All replicas independently run watchers + gRPC; the Leader additionally handles status + ACME | High availability, gRPC load balancing required |

### Configuration

```yaml
conf_center:
  type: kubernetes
  gateway_class: edgion
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
|  | PROCESSOR_REGISTRY    Y |              |  | PROC_REGISTRY  Y | | PROC_REGISTRY  Y | |
|  | ConfigSyncServer     Y |              |  | ConfigSyncSrv  Y | | ConfigSyncSrv  Y | |
|  | gRPC List/Watch      Y |              |  | gRPC List/Watch Y | | gRPC List/Watch Y | |
|  | Status -> K8s API    Y |              |  | Status -> K8s  Y | | Status -> K8s  N | |
|  | ACME                 Y |              |  | ACME           Y | | ACME           N | |
|  +-------------------------+              |  +------------------+ +------------------+ |
|                                           |                                            |
|  Pod B (standby)                          |  Gateway -> any replica                    |
|  +-------------------------+              |  (edgion-controller Service)               |
|  | (completely idle)       |              |                                            |
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
| ACME certificate automation | Leader (implicit; the entire main_flow runs only on leader) | Leader (explicit guard, `start_leader_services()`) |
| Leadership-loss handling | Exit `run_main_flow`, clear everything, return to `wait_until_leader` | Only `stop_leader_services()`; keep serving gRPC |
| Gateway connection target | `edgion-controller-leader` Service | `edgion-controller` (all-pods) Service |
| gRPC interruption on leader switchover | Yes (cache rebuild ~5–15s) | No (other replicas keep serving) |
| K8s API server load | 1x watcher connection | Nx watcher connections (N = replica count) |
| Memory footprint | Only the Leader uses cache memory | All replicas use cache memory |

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

When `leader_handle` is `None` (FileSystem mode or no HA), `is_none_or` returns `true` and writes are always allowed.

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
└── PROCESSOR_REGISTRY.requeue_all()    // full-status reconciliation

stop_leader_services():
└── stop_acme_service()
```

## Leader-Switchover Reconciliation (requeue_all)

After a new Leader takes over, it calls `PROCESSOR_REGISTRY.requeue_all()` to trigger a full status recomputation and writeback.

**Why reconciliation is needed**: the previous Leader may have left stale status before losing leadership;
the new Leader's cache may differ from the status currently in the K8s API.
A full requeue ensures all resources' status is consistent with the new Leader's cache.

`requeue_all()` walks every registered processor in `PROCESSOR_REGISTRY`,
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
| leader-only | `server_addr = "http://edgion-controller-leader.edgion-system.svc:12151"` | Connect only to the leader |
| all-serve | `server_addr = "http://edgion-controller.edgion-system.svc:12151"` | Any replica |

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
| `edgion-controller-leader` | `app: edgion-controller` + `edgion.io/leader: "true"` | Admin API reload, ACME callback |

## /ready Endpoint Semantics

`is_ready()` checks `PROCESSOR_REGISTRY.is_all_ready() && config_sync_server.is_some()`:

- **leader-only**: non-leaders return not ready (no services started); they are not added to Service endpoints
- **all-serve**: every ready replica returns ready and can receive Gateway traffic

## Migration Paths

### Single replica → HA (leader-only)

1. Deploy `leader-service.yaml` (with the `edgion.io/leader: "true"` selector) and RBAC (Pod patch permissions)
2. Point Gateway `server_addr` at `edgion-controller-leader`
3. Set `replicas: 2`

### leader-only → all-serve

1. Change `ha_mode = "all-serve"`
2. Point Gateway `server_addr` at `edgion-controller` (all-pods Service)
3. Rolling restart

### Rollback all-serve → leader-only

1. Change `ha_mode = "leader-only"`
2. Point Gateway `server_addr` at `edgion-controller-leader`
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
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/config.rs` — `HaMode` enum
