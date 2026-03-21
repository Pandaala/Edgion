---
name: kubernetes-ha-mode
description: Kubernetes 高可用模式：leader-only vs all-serve 行为矩阵、架构图、status 回写守卫、ACME leader-only、leader 切换对账。
---

# Kubernetes HA 模式

通过 `ha_mode` 配置项支持两种多副本运行模式，满足不同规模和可用性需求。

## 模式概述

| Mode | 名称 | 描述 | 适用场景 |
|------|------|------|----------|
| `leader-only` | Leader-Only（默认） | 仅 Leader 运行 watchers + gRPC；非 Leader 完全待机 | 小规模部署、资源敏感 |
| `all-serve` | All Replicas Serve | 所有副本独立运行 watchers + gRPC；Leader 额外负责 status + ACME | 高可用要求高、需要 gRPC 负载均衡 |

### 配置

```toml
[conf_center]
type = "kubernetes"
gateway_class = "edgion"
ha_mode = "leader-only"    # or "all-serve"

[conf_center.leader_election]
lease_name = "edgion-controller-leader"
lease_namespace = "edgion-system"
```

## 架构对比

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

## 行为矩阵

| 行为 | `leader-only` | `all-serve` |
|------|---------------|-------------|
| K8s watchers 启动时机 | 仅 Leader | 所有副本（立即启动） |
| gRPC 可用性 | Leader 就绪后 | 任意副本就绪后 |
| `set_config_sync_server(Some)` | Leader + CachesReady | CachesReady（任意副本） |
| Status 回写 K8s | Leader（`leader_handle` 守卫） | Leader（`leader_handle` 守卫） |
| ACME 证书自动化 | Leader（隐式，整个 main_flow 只在 leader 运行） | Leader（显式守卫，`start_leader_services()`） |
| Leadership 丢失处理 | 退出 `run_main_flow`，清空一切，回到 `wait_until_leader` | 仅 `stop_leader_services()`，继续 gRPC 服务 |
| Gateway 连接目标 | `edgion-controller-leader` Service | `edgion-controller`（all pods）Service |
| Leader 切换时的 gRPC 中断 | 有（cache 重建约 5-15s） | 无（其他副本继续服务） |
| K8s API server 负载 | 1x watcher 连接 | Nx watcher 连接（N = 副本数） |
| 内存占用 | 仅 Leader 占用 cache 内存 | 所有副本占用 cache 内存 |

## Status 回写 Leader 守卫

所有副本都通过 `ResourceProcessor` 计算 status（保证 gRPC cache 数据正确），
但只有 Leader 写回 K8s API。守卫逻辑：

```rust
let can_write_status = leader_handle.as_ref().is_none_or(|h| h.is_leader());
if status_changed && can_write_status {
    persist_k8s_status::<K>(...).await;
}
```

此守卫在三个检查点一致应用：
1. **Init phase** — `Event::InitApply` 处理中
2. **Init phase edge case** — `Event::Apply` 在 `init_done` 之前到达时
3. **Runtime worker** — `spawn_worker` 中的 work item 处理

当 `leader_handle` 为 `None`（FileSystem 模式或无 HA）时，`is_none_or` 返回 `true`，始终允许写入。

### 为什么不能所有副本都写 status

如果所有副本都写 status，会引发无限循环：

```
Pod-A 计算 ResolvedRefs=True → 写入 K8s
  → Pod-B cache 尚未同步该 Secret → 计算 ResolvedRefs=False → 覆盖 K8s
    → watch event 回弹给所有副本 → 重新处理 → 循环...
```

## ACME 必须 Leader-Only

ACME HTTP-01 challenge 要求单点请求证书 + 单点响应验证。
多副本同时请求会导致 challenge 冲突和证书签发失败。

- **leader-only 模式**：ACME 隐式只在 leader 运行（整个 main_flow 只在 leader 执行）
- **all-serve 模式**：`start_acme_service()` 仅在 `start_leader_services()` 中调用

```rust
start_leader_services():
├── start_acme_service(client)
└── PROCESSOR_REGISTRY.requeue_all()    // 全量 status 对账

stop_leader_services():
└── stop_acme_service()
```

## Leader 切换对账（requeue_all）

新 Leader 上任后调用 `PROCESSOR_REGISTRY.requeue_all()`，触发全量 status 重算和回写。

**为什么需要对账**：旧 Leader 可能在失去 leadership 前遗留了 stale status，
新 Leader 的 cache 可能与 K8s API 中的 status 存在差异。
全量 requeue 确保所有资源的 status 与新 Leader 的 cache 一致。

`requeue_all()` 遍历 `PROCESSOR_REGISTRY` 中所有已注册的处理器，
从各自的 `ServerCache` 获取所有 key 并异步 enqueue 到 workqueue。
Workqueue 的去重机制确保不会产生重复处理。

## server_id 与 Gateway 重连

每个副本有独立的 `ConfigSyncServer` 实例，生成不同的 `server_id`。

Gateway 在 gRPC Watch 流中检测 `server_id` 变化：
- **leader-only 模式**：leader 切换后，新 leader 创建新 ConfigSyncServer（新 server_id），
  Gateway 检测到变化后执行 full relist
- **all-serve 模式**：Gateway 连接切换到另一副本时，检测到不同 server_id，执行 full relist

这是正确的设计行为：确保 Gateway 与当前 Controller 副本的数据完全一致。

## Gateway 连接目标

| HA 模式 | Gateway 配置 | 说明 |
|---------|-------------|------|
| leader-only | `server_addr = "http://edgion-controller-leader.edgion-system.svc:50051"` | 只连 leader |
| all-serve | `server_addr = "http://edgion-controller.edgion-system.svc:50051"` | 任意副本 |

## K8s Service 拓扑

### leader-only 模式

| Service | Selector | 用途 |
|---------|----------|------|
| `edgion-controller` | `app: edgion-controller` | health check、metrics |
| `edgion-controller-leader` | `app: edgion-controller` + `edgion.io/leader: "true"` | Gateway gRPC、Admin API |

### all-serve 模式

| Service | Selector | 用途 |
|---------|----------|------|
| `edgion-controller` | `app: edgion-controller` | Gateway gRPC、health check、metrics |
| `edgion-controller-leader` | `app: edgion-controller` + `edgion.io/leader: "true"` | Admin API reload、ACME callback |

## /ready Endpoint 语义

`is_ready()` 检查 `PROCESSOR_REGISTRY.is_all_ready() && config_sync_server.is_some()`：

- **leader-only**：非 Leader 返回 not ready（未启动任何服务），不进入 Service endpoints
- **all-serve**：所有就绪副本返回 ready，都能接收 Gateway 流量

## 迁移路径

### 单副本 → HA（leader-only）

1. 部署 `leader-service.yaml`（带 `edgion.io/leader: "true"` selector）+ RBAC（Pod patch 权限）
2. Gateway `server_addr` 指向 `edgion-controller-leader`
3. 设置 `replicas: 2`

### leader-only → all-serve

1. 修改 `ha_mode = "all-serve"`
2. Gateway `server_addr` 指向 `edgion-controller`（all-pods Service）
3. Rolling restart

### 回滚 all-serve → leader-only

1. 修改 `ha_mode = "leader-only"`
2. Gateway `server_addr` 指向 `edgion-controller-leader`
3. Rolling restart

### RBAC 要求

Pod label patch 需要额外权限：

```yaml
rules:
- apiGroups: [""]
  resources: ["pods"]
  verbs: ["get", "patch"]
```

## 关键文件

- `src/core/controller/conf_mgr/conf_center/kubernetes/center.rs` — `run_leader_only_lifecycle`, `run_all_serve_lifecycle`, `run_main_flow`, `run_serving_flow`
- `src/core/controller/conf_mgr/conf_center/kubernetes/leader_election.rs` — `LeaderElection`, `LeaderHandle`
- `src/core/controller/conf_mgr/conf_center/kubernetes/config.rs` — `HaMode` 枚举
