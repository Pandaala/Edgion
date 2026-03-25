# Data-Plane Status Feedback — Programmed & Ready Conditions

> Controller 通过向 Gateway 查询数据面状态，回填 Programmed 和 Ready conditions 到 CRD status。
> Priority: P2 (feature enhancement)

## Background

2026-03-17 移除了 Programmed 和 Ready conditions，因为从控制面直接设置这些 condition 是误导性的。
这些 condition 需要**数据面反馈**才能准确：

| Condition | 含义 | 数据来源 |
|-----------|------|----------|
| Programmed | 配置已编程到数据面 | Gateway 是否已加载并应用该资源的配置 |
| Ready | 资源就绪可处理流量 | Gateway 的运行时 store 中该资源是否可用 |

当前架构 Controller → Gateway 是单向 gRPC 推送（List/Watch），没有反向状态通道。

## Scope

### In Scope

- Controller 从任意一个 Gateway 查询资源的数据面加载状态
- 延时查询队列：资源变更 (add/update) 后延时 10s 查询
- 最多 1-2 次重试，失败即放弃（不循环）
- 支持的资源类型：
  - Gateway API routes: HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute
  - Custom CRDs: EdgionTls, EdgionPlugins, EdgionStreamPlugins, EdgionGatewayConfig, LinkSys, BackendTLSPolicy, Gateway
- 回填 Programmed 和 Ready conditions 到 K8s CRD status

### Out of Scope

- Service、Endpoint、EndpointSlice、Secret、ConfigMap（K8s 原生资源，不设 status）
- ReferenceGrant、PluginMetaData（无 status 或不同步到 Gateway）
- 所有 Gateway 实例的一致性验证（任选一个即可，Deployment 行为一致）
- Gateway Admin API 的认证鉴权（集群内部通信）

## Steps

| Step | Content | Status |
|------|---------|--------|
| step-01 | 架构分析与方案选型 | completed |
| step-02 | 详细设计 | completed |
| step-03 | 实现计划（文件级） | completed |
| step-04 | 实现 | pending |
| step-05 | 测试 | pending |

---

## Step 01: Architecture Analysis & Approach Selection

### Current Architecture

```
Controller (gRPC Server)                    Gateway (gRPC Client)
  ┌─────────────────────┐                   ┌─────────────────────┐
  │ ConfigSyncServer     │ ◄─── Watch ───── │ ConfigSyncClient    │
  │  ├ WatchObj per kind │ ◄─── List ──────  │  ├ ClientCache<T>  │
  │  └ ClientRegistry   │ ◄─ WatchMeta ──── │  └ ConfHandler      │
  │                      │                   │                      │
  │ No knowledge of      │                   │ Admin API (axum)    │
  │ Gateway pod IPs      │                   │  ├ /health          │
  │                      │                   │  ├ /ready           │
  └─────────────────────┘                   │  ├ /configclient/   │
                                             │  └ /store-stats    │
                                             └─────────────────────┘
```

Key constraints:
- Controller 不知道 Gateway pod 地址（只通过 `ClientRegistry` 记录 client_id 和 count）
- Gateway 通过 `--server-addr` 静态配置连接 Controller
- Gateway Admin API 在每个 pod 的 HTTP 端口上（默认 5900）

### Three Approaches Evaluated

#### Approach A: Controller → Gateway HTTP Pull (Recommended)

Controller 主动调用 Gateway Admin API 查询资源加载状态。

```
Controller                                 Gateway
  │                                          │
  │  1. Resource updated (via Watch/K8s)     │
  │  2. update_status: set Accepted,         │
  │     ResolvedRefs                          │
  │  3. Enqueue delayed status query (10s)   │
  │        ...10s later...                    │
  │  4. GET /api/v1/status-query?kind=X&     │
  │     namespace=Y&name=Z ──────────────────>│
  │  5.                   <── { programmed:   │
  │                           true, ready:    │
  │                           true }          │
  │  6. Patch Programmed: True, Ready: True  │
  │     to K8s status                         │
```

**Pros:**
- Gateway Admin API 已存在，只需添加一个新 endpoint
- Controller 主动控制查询时机和频率
- 无需修改 gRPC proto
- 查询逻辑与 config sync 完全解耦

**Cons:**
- Controller 需要发现至少一个 Gateway 地址
- 需要新增 Gateway 地址发现机制

**Gateway address discovery**: 利用已有的 `ClientRegistry` 扩展，在 `WatchServerMeta` 注册时
携带 Gateway 的 admin API 地址。这是最小改动方案。

#### Approach B: Gateway → Controller gRPC Push

Gateway 通过新增的 gRPC RPC 主动推送数据面状态。

```
Controller                                 Gateway
  │                                          │
  │  ◄── ReportStatus(kind, key, status) ── │
  │                                          │
```

**Pros:**
- Gateway 主动推送，Controller 被动接收
- 不需要 Controller 发现 Gateway 地址

**Cons:**
- 需要修改 gRPC proto（新增 RPC）
- Gateway 需要知道何时"该汇报了"——需要一个 trigger 机制
- 多个 Gateway 实例会重复上报（需要去重）
- proto 兼容性管理更复杂

#### Approach C: Piggyback on Watch Stream

在现有 Watch stream 中嵌入反向状态信息。

**Cons:**
- 严重违反单一职责（Watch 是 config sync，不是 status）
- WatchResponse 结构需要改动
- 增加 Watch stream 的复杂度和 debug 难度

**REJECTED**: 架构不干净，与现有设计理念冲突。

### Decision: Approach A (HTTP Pull)

选择 Approach A 的核心理由：
1. **最小改动原则**：Gateway 已有 Admin API 框架，只需添加 endpoint
2. **Controller 主导**：Controller 决定查询时机，安全可控
3. **解耦**：status 查询与 config sync 完全独立
4. **Gateway 地址发现**：扩展 `ClientRegistry` 是自然演进，改动小
5. 用户原话 "controller 通过 grpc 反向去向 gateway 获取" 与 pull 模式对齐

### Risks

- **Gateway 不可达**：Controller 查询超时，条件不设置（保持只有 Accepted/ResolvedRefs）
- **状态不一致窗口**：资源变更后 10s 内 Programmed/Ready 缺失，这是 by design
- **Gateway 滚动升级中**：不同版本可能返回不同结果，但查询任意一个即可

### Need Confirmation

- [ ] Gateway Admin API 端口是否始终可达？（集群内 Pod IP 直连）
- [ ] 10s 延时是否合适？可配置？
- [ ] 失败后不设置 Programmed/Ready（而不是设为 False）是否可接受？

---

## Step 02: Detailed Design

### 2.1 Gateway Side: Status Query Endpoint

**New endpoint**: `GET /api/v1/data-plane-status`

```
GET /api/v1/data-plane-status?kind=HTTPRoute&namespace=default&name=my-route
```

Response:

```json
{
  "success": true,
  "data": {
    "programmed": true,
    "ready": true,
    "details": {
      "in_config_cache": true,
      "in_runtime_store": true
    }
  }
}
```

**Status determination logic per resource kind**:

| Kind | Programmed = True when | Ready = True when |
|------|----------------------|-------------------|
| HTTPRoute | `HttpRouteManager` has matching route entries | Same as Programmed (no separate readiness) |
| GRPCRoute | `GrpcRouteManager` has matching route entries | Same |
| TCPRoute | `GlobalTcpRouteManagers` has matching route entries | Same |
| TLSRoute | `TlsRouteManager` has matching route entries | Same |
| UDPRoute | `GlobalUdpRouteManagers` has matching route entries | Same |
| EdgionTls | `TlsStore` has the TLS entry AND `CertMatcher` has it | Same |
| EdgionPlugins | `PluginStore` has the plugin | Same |
| EdgionStreamPlugins | `StreamPluginStore` has the plugin | Same |
| EdgionGatewayConfig | `EdgionGatewayConfigStore` has the config | Same |
| LinkSys | `LinkSysStore` has the resource | Same |
| BackendTLSPolicy | `BackendTLSPolicyStore` has the policy | Same |
| Gateway | `GatewayConfigStore` has the gateway AND listeners are bound | Listeners are actively serving |

**Implementation in Gateway**:

```rust
// New file: src/core/gateway/api/status_query.rs

use crate::types::ResourceKind;

#[derive(Serialize)]
pub struct DataPlaneStatus {
    pub programmed: bool,
    pub ready: bool,
}

pub async fn query_data_plane_status(
    kind: ResourceKind,
    namespace: Option<&str>,
    name: &str,
) -> DataPlaneStatus {
    match kind {
        ResourceKind::HTTPRoute => {
            let mgr = get_global_route_manager();
            let exists = mgr.has_route(namespace, name);
            DataPlaneStatus { programmed: exists, ready: exists }
        }
        ResourceKind::EdgionTls => {
            let store = get_global_tls_store();
            let exists = store.has_entry(namespace, name);
            DataPlaneStatus { programmed: exists, ready: exists }
        }
        // ... other kinds
        _ => DataPlaneStatus { programmed: false, ready: false },
    }
}
```

**需要在各个 runtime store 中添加的查询方法**:

| Store | New Method | File |
|-------|-----------|------|
| `HttpRouteManager` | `has_route(ns, name) -> bool` | `src/core/gateway/routes/http/mod.rs` |
| `GrpcRouteManager` | `has_route(ns, name) -> bool` | `src/core/gateway/routes/grpc/mod.rs` |
| `GlobalTcpRouteManagers` | `has_route(ns, name) -> bool` | `src/core/gateway/routes/tcp/mod.rs` |
| `TlsRouteManager` | `has_route(ns, name) -> bool` | `src/core/gateway/routes/tls/mod.rs` |
| `GlobalUdpRouteManagers` | `has_route(ns, name) -> bool` | `src/core/gateway/routes/udp/mod.rs` |
| `TlsStore` | `has_entry(ns, name) -> bool` | `src/core/gateway/tls/store/tls_store.rs` |
| `PluginStore` | `has_plugin(ns, name) -> bool` | `src/core/gateway/plugins/http/mod.rs` |
| `StreamPluginStore` | `has_plugin(ns, name) -> bool` | `src/core/gateway/plugins/stream/mod.rs` |
| `LinkSysStore` | `has_resource(ns, name) -> bool` | `src/core/gateway/link_sys/runtime/store.rs` |
| `GatewayConfigStore` | `has_gateway(ns, name) -> bool` | `src/core/gateway/runtime/store/config.rs` |
| `BackendTLSPolicyStore` | `has_policy(ns, name) -> bool` | `src/core/gateway/backends/policy.rs` |
| `EdgionGatewayConfigStore` | `has_config(name) -> bool` | `src/core/gateway/config/edgion_gateway.rs` |

### 2.2 Gateway Discovery: ClientRegistry Extension

Extend `WatchServerMetaRequest` to carry Gateway admin address:

```protobuf
// config_sync.proto — MODIFIED
message WatchServerMetaRequest {
    string client_id = 1;
    string client_name = 2;
    string admin_addr = 3;    // NEW: Gateway admin API address (e.g., "10.0.1.5:5900")
}
```

Extend `ClientRegistry` to store admin address:

```rust
// client_registry.rs — MODIFIED
struct ClientMeta {
    client_name: String,
    connected_at: SystemTime,
    admin_addr: Option<String>,  // NEW
}

impl ClientRegistry {
    // NEW: Get an arbitrary connected gateway's admin address
    pub fn any_admin_addr(&self) -> Option<String> {
        self.clients.read().unwrap()
            .values()
            .find_map(|meta| meta.admin_addr.clone())
    }
}
```

Gateway startup sends its admin address:

```rust
// grpc_client.rs — MODIFIED
pub async fn start_watch_server_meta(self: Arc<Self>) {
    // ...
    .watch_server_meta(WatchServerMetaRequest {
        client_id: client_id.clone(),
        client_name: client_name.clone(),
        admin_addr: self.admin_addr.clone(),  // e.g., "POD_IP:5900"
    })
}
```

### 2.3 Controller Side: Delayed Status Query Queue

**New module**: `src/core/controller/conf_mgr/sync_runtime/status_query/`

```
status_query/
├── mod.rs           // Module exports
├── queue.rs         // StatusQueryQueue implementation
├── worker.rs        // Background worker that processes the queue
└── client.rs        // HTTP client for Gateway Admin API
```

#### StatusQueryQueue

```rust
// queue.rs
use std::collections::{HashMap, BinaryHeap};
use tokio::sync::{mpsc, Notify};

pub struct StatusQueryQueue {
    // Delayed items in a min-heap (earliest first)
    heap: Mutex<BinaryHeap<Reverse<DelayedQuery>>>,
    // Dedup set: prevents same resource from being queued multiple times
    pending: Mutex<HashSet<String>>,
    // Notify worker when new items arrive
    notify: Notify,
    // Configuration
    config: StatusQueryConfig,
}

pub struct StatusQueryConfig {
    pub delay: Duration,            // Default: 10s
    pub max_retries: u32,           // Default: 2
    pub query_timeout: Duration,    // Default: 3s
    pub retry_delay: Duration,      // Default: 5s
}

#[derive(Eq, PartialEq)]
struct DelayedQuery {
    key: String,          // "namespace/name"
    kind: String,         // "HTTPRoute"
    ready_at: Instant,
    attempt: u32,
    generation: i64,      // Resource generation at time of enqueue
}

impl StatusQueryQueue {
    /// Schedule a status query after the configured delay.
    /// Returns false if already pending (dedup).
    pub fn schedule(&self, kind: &str, namespace: &str, name: &str, generation: i64) -> bool {
        let key = format!("{}/{}/{}", kind, namespace, name);
        let mut pending = self.pending.lock().unwrap();
        if pending.contains(&key) {
            return false;
        }
        pending.insert(key.clone());

        let mut heap = self.heap.lock().unwrap();
        heap.push(Reverse(DelayedQuery {
            key,
            kind: kind.to_string(),
            ready_at: Instant::now() + self.config.delay,
            attempt: 0,
            generation,
        }));
        drop(heap);
        drop(pending);
        self.notify.notify_one();
        true
    }

    /// Clear all pending queries (on controller reload/re-election)
    pub fn clear(&self) {
        self.heap.lock().unwrap().clear();
        self.pending.lock().unwrap().clear();
    }
}
```

#### Worker Loop

```rust
// worker.rs
pub async fn run_status_query_worker(
    queue: Arc<StatusQueryQueue>,
    client_registry: Arc<ClientRegistry>,
    k8s_client: Client,
) {
    loop {
        // Wait for next item to be ready
        let item = queue.dequeue_when_ready().await;  // blocks until ready_at

        // Get any gateway address
        let Some(gateway_addr) = client_registry.any_admin_addr() else {
            // No gateway connected, discard query
            queue.mark_done(&item.key);
            continue;
        };

        // Query gateway
        let result = query_gateway_status(
            &gateway_addr, &item.kind, /* namespace */, /* name */,
            queue.config.query_timeout,
        ).await;

        match result {
            Ok(status) => {
                // Build Programmed and Ready conditions
                let conditions = build_conditions(status, item.generation);
                // Patch K8s status
                patch_status_conditions(&k8s_client, &item.kind, /* ns */, /* name */, conditions).await;
                queue.mark_done(&item.key);
            }
            Err(e) => {
                if item.attempt < queue.config.max_retries {
                    // Retry with delay
                    queue.retry(item, queue.config.retry_delay);
                } else {
                    // Give up — leave Programmed/Ready unset
                    tracing::warn!(
                        kind = %item.kind,
                        key = %item.key,
                        attempts = item.attempt + 1,
                        error = %e,
                        "Status query failed after max retries, giving up"
                    );
                    queue.mark_done(&item.key);
                }
            }
        }
    }
}
```

### 2.4 Integration: Triggering Status Queries

Status queries are triggered in `resource_controller.rs` after `persist_k8s_status`:

```rust
// resource_controller.rs — MODIFIED (worker loop)
if let WorkItemResult::Processed { obj, status_changed } = result {
    // ... existing status persistence ...

    // Schedule delayed data-plane status query
    if self.status_query_queue.is_some() && is_status_queryable_kind::<K>() {
        let name = obj.meta().name.as_deref().unwrap_or("");
        let namespace = obj.meta().namespace.as_deref().unwrap_or("default");
        let generation = obj.meta().generation.unwrap_or(0);
        self.status_query_queue.as_ref().unwrap().schedule(
            K::kind(&K::DynamicType::default()).as_ref(),
            namespace,
            name,
            generation,
        );
    }
}
```

`is_status_queryable_kind()` whitelist:

```rust
fn is_status_queryable_kind<K: Resource>() -> bool {
    let kind = K::kind(&K::DynamicType::default());
    matches!(kind.as_ref(),
        "HTTPRoute" | "GRPCRoute" | "TCPRoute" | "TLSRoute" | "UDPRoute" |
        "EdgionTls" | "EdgionPlugins" | "EdgionStreamPlugins" |
        "EdgionGatewayConfig" | "LinkSys" | "BackendTLSPolicy" | "Gateway"
    )
}
```

### 2.5 Status Patching Strategy

When the status query succeeds, we patch **only** the Programmed and Ready conditions
into the existing status, not replace the whole status:

```rust
async fn patch_status_conditions(
    client: &Client,
    kind: &str,
    namespace: &str,
    name: &str,
    programmed: bool,
    ready: bool,
    generation: i64,
) {
    // Read-modify-write pattern:
    // 1. Read current status from cache (not K8s API — avoid unnecessary reads)
    // 2. Add/update Programmed and Ready conditions
    // 3. JSON Merge Patch to K8s status subresource
    //
    // For per-parent resources (routes, EdgionTls), patch into each parent's conditions.
    // For simple resources, patch into status.conditions directly.
}
```

**Critical**: The generation check ensures we don't set Programmed: True for a stale version.
If the resource was updated between enqueue and query, the generation won't match and we discard.

### 2.6 Cycle Prevention & Safety

| Safety measure | Implementation |
|----------------|---------------|
| **No infinite loops** | `max_retries = 2`, after that give up permanently |
| **Dedup** | `pending` HashSet prevents same resource from double-queuing |
| **Generation check** | Query result only applied if resource generation matches |
| **Clear on reload** | `queue.clear()` called on controller reload/re-election |
| **Gateway unavailable** | Query skipped if no Gateway registered |
| **Timeout** | HTTP query timeout = 3s |
| **No write amplification** | Status query ONLY adds Programmed/Ready; does not trigger re-processing or re-enqueue |
| **Leader guard** | Only leader controller runs the worker |

### Risks

- **Race condition**: Resource updated again while query is pending. Mitigated by generation check.
- **Gateway cold start**: Gateway not yet connected when query fires. Mitigated by discarding.
- **Status patch conflict**: Another writer (e.g., re-processing) patches status concurrently.
  Kubernetes Merge Patch is last-writer-wins; since we only write Programmed/Ready and
  the main flow writes Accepted/ResolvedRefs, they operate on disjoint condition types.

### Need Confirmation

- [ ] HTTP client in Controller — prefer reqwest or hyper? (reqwest is simpler)
- [ ] Gateway admin port: is it always 5900? Should it be configurable?
- [ ] Per-parent resources: should Programmed/Ready be set per-parent or at resource level?
  (Gateway API spec says per-parent for route status; recommend per-parent for consistency)

---

## Step 03: Implementation Plan

### Phase 1: Gateway Side (Status Query Endpoint)

Estimated: ~3-4 files modified, 1 new file

| # | File | Change | Effort |
|---|------|--------|--------|
| 1.1 | `src/core/gateway/api/status_query.rs` (NEW) | Status query handler and per-kind lookup logic | Medium |
| 1.2 | `src/core/gateway/api/mod.rs` | Add `/api/v1/data-plane-status` route | Small |
| 1.3 | Route managers: `routes/http/mod.rs`, `routes/grpc/mod.rs`, `routes/tcp/mod.rs`, `routes/tls/mod.rs`, `routes/udp/mod.rs` | Add `has_route(ns, name) -> bool` | Small each |
| 1.4 | Stores: `tls/store/tls_store.rs`, `plugins/http/mod.rs`, `plugins/stream/mod.rs`, `link_sys/runtime/store.rs`, `runtime/store/config.rs`, `backends/policy.rs`, `config/edgion_gateway.rs` | Add `has_entry/has_plugin/has_resource(ns, name) -> bool` | Small each |

### Phase 2: Gateway Discovery (Proto + Registry)

Estimated: ~4 files modified

| # | File | Change | Effort |
|---|------|--------|--------|
| 2.1 | `src/core/common/conf_sync/proto/config_sync.proto` | Add `admin_addr` field to `WatchServerMetaRequest` | Small |
| 2.2 | `src/core/controller/conf_sync/conf_server/client_registry.rs` | Store admin_addr in `ClientMeta`; add `any_admin_addr()` | Small |
| 2.3 | `src/core/controller/conf_sync/conf_server/grpc_server.rs` | Pass admin_addr to registry.register() | Small |
| 2.4 | `src/core/gateway/conf_sync/conf_client/grpc_client.rs` | Send admin_addr in WatchServerMetaRequest | Small |
| 2.5 | `src/core/gateway/cli/mod.rs` or config | Determine and expose admin addr (POD_IP + port) | Small |

### Phase 3: Controller Side (Queue + Worker + Client)

Estimated: 1 new module (3-4 files), 2-3 files modified

| # | File | Change | Effort |
|---|------|--------|--------|
| 3.1 | `src/core/controller/conf_mgr/sync_runtime/status_query/mod.rs` (NEW) | Module exports | Small |
| 3.2 | `src/core/controller/conf_mgr/sync_runtime/status_query/queue.rs` (NEW) | StatusQueryQueue (delay heap + dedup) | Medium |
| 3.3 | `src/core/controller/conf_mgr/sync_runtime/status_query/worker.rs` (NEW) | Worker loop + retry logic | Medium |
| 3.4 | `src/core/controller/conf_mgr/sync_runtime/status_query/client.rs` (NEW) | HTTP client for Gateway status API | Small |
| 3.5 | `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs` | Inject StatusQueryQueue; schedule queries after processing | Medium |
| 3.6 | `src/core/controller/cli/mod.rs` | Create and start status query worker | Small |

### Phase 4: Status Utils & Handlers (Re-introduce Programmed/Ready)

Estimated: ~8 files modified

| # | File | Change | Effort |
|---|------|--------|--------|
| 4.1 | `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs` | Re-add `PROGRAMMED` and `READY` condition types/reasons; add `programmed_condition()` and `ready_condition()` | Small |
| 4.2 | `src/core/controller/conf_mgr/sync_runtime/resource_processor/mod.rs` | Re-export `programmed_condition`, `ready_condition` | Small |
| 4.3 | `src/core/controller/conf_mgr/sync_runtime/status_query/worker.rs` | Build and patch Programmed/Ready conditions using status_utils | Medium |

Note: Handlers do NOT set Programmed/Ready themselves. Only the status query worker sets them
based on Gateway feedback. This is the key architectural difference from the old approach.

### Phase 5: CRD & Skill Updates

| # | File | Change | Effort |
|---|------|--------|--------|
| 5.1 | All CRD YAML files with status conditions | Update description to include Programmed and Ready again | Small |
| 5.2 | `skills/01-architecture/01-controller/10-status-management.md` | Update to reflect data-plane feedback mechanism | Small |
| 5.3 | `skills/01-architecture/02-grpc-sync.md` | Add status query section | Small |

### Phase 6: Testing

| # | Type | Description |
|---|------|-------------|
| 6.1 | Unit test | `StatusQueryQueue`: schedule, dedup, retry, clear, generation check |
| 6.2 | Unit test | `status_query.rs` (Gateway): per-kind status lookup |
| 6.3 | Unit test | `ClientRegistry`: admin_addr storage and retrieval |
| 6.4 | Integration test | End-to-end: create HTTPRoute → wait 10s → verify Programmed/Ready in status |

### Dependency Graph

```
Phase 1 (Gateway endpoint) ──┐
Phase 2 (Gateway discovery) ──┼── Phase 3 (Controller queue/worker) ── Phase 4 (Re-add conditions)
                               │                                            │
                               └── Phase 5 (CRD/Skill updates) ────────────┘
                                                                    │
                                                            Phase 6 (Testing)
```

Phase 1 and Phase 2 can be done in parallel.
Phase 3 depends on Phase 1 (endpoint exists) and Phase 2 (address discovery).
Phase 4 depends on Phase 3.
Phase 5 and Phase 6 depend on Phase 4.

### New Dependency

- `reqwest` (HTTP client for Controller → Gateway queries)
  - Already likely in the dependency tree; if not, add with `features = ["json"]`
  - Alternative: use `hyper` directly (more manual but no new dependency)

---

## Step 04–06: Pending

To be filled during implementation.

---

## Review Notes

### Architectural Consistency

The design maintains the existing single-direction gRPC config sync (Controller → Gateway)
and adds a separate, orthogonal HTTP status query path (Controller → Gateway). These two
communication channels serve completely different purposes:

- **gRPC**: Real-time config distribution (watch-based, streaming)
- **HTTP**: Point-in-time status queries (request-response, delayed, best-effort)

### Gateway API Compliance

Per Gateway API specification (GEP-1364):
- `Programmed` should be set when the configuration has been received and processed by the data plane
- `Ready` should be set when the data plane is actively serving traffic based on the configuration

The proposed design correctly ties these conditions to actual data-plane state rather than
control-plane assumptions.

### Comparison with Alternative Approaches

| Dimension | A: HTTP Pull (chosen) | B: gRPC Push | C: Watch Piggyback |
|-----------|----------------------|-------------|-------------------|
| Controller needs Gateway address | Yes (via registry) | No | No |
| Proto change | Minimal (1 field) | Major (new RPC + message) | Major (WatchResponse change) |
| Multi-instance handling | Pick any one | Need dedup | Complex |
| Separation of concerns | Clean | Clean | Poor |
| Trigger mechanism | Controller-driven | Gateway needs trigger | Unclear |
| Implementation complexity | Medium | Medium-High | High |
| Testability | Easy (mock HTTP) | Medium | Hard |

### Safety Analysis

| Threat | Mitigation |
|--------|-----------|
| Infinite query loops | max_retries = 2, then permanent give-up |
| Resource version mismatch | generation check before patching |
| Gateway down during query | 3s timeout, discard result |
| Controller reload | queue.clear() on reload |
| Leader switch | Only leader runs worker; queue cleared on re-election |
| Status patch conflicts | Disjoint condition types (main flow: Accepted/ResolvedRefs; query worker: Programmed/Ready) |
| Memory leak in queue | `pending` set cleaned up on mark_done; `clear()` on reload |

## Related

- [09-status-management.md](../../skills/01-architecture/01-controller/10-status-management.md) — Status management guidelines
- [02-grpc-sync.md](../../skills/01-architecture/02-grpc-sync.md) — gRPC sync architecture
- [resource_controller.rs](../../src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs) — Status persistence
- [client_registry.rs](../../src/core/controller/conf_sync/conf_server/client_registry.rs) — Gateway tracking
- [Gateway Admin API](../../src/core/gateway/api/mod.rs) — Existing admin endpoints
