# Controller HA: Mode-Based Architecture

> 通过 `ha_mode` 配置项支持两种多副本运行模式。

## HA Mode 概述

| Mode | 名称 | 描述 | 适用场景 |
|------|------|------|----------|
| `leader-only` | Leader-Only（默认） | 仅 Leader 运行 watchers + gRPC；非 Leader 完全待机 | 小规模部署、刚开始多副本 |
| `all-serve` | All Replicas Serve | 所有副本独立运行 watchers + gRPC；Leader 额外负责 status + ACME | 高可用要求高、需要 gRPC 负载均衡 |

### 配置示例

```toml
[conf_center]
type = "kubernetes"
gateway_class = "edgion"
ha_mode = "leader-only"    # or "all-serve"

[conf_center.leader_election]
lease_name = "edgion-controller-leader"
lease_namespace = "edgion-system"
```

## 模式对比

```
┌─────────────────────────────────┬──────────────────────────────────────┐
│  leader-only (default)          │  all-serve                           │
├─────────────────────────────────┼──────────────────────────────────────┤
│                                 │                                      │
│  Pod A (leader)                 │  Pod A (leader)    Pod B (non-leader)│
│  ┌─────────────────────┐       │  ┌──────────────┐ ┌──────────────┐  │
│  │ K8s watchers      ✓ │       │  │ K8s watchers ✓│ │ K8s watchers ✓│ │
│  │ PROCESSOR_REGISTRY ✓│       │  │ PROC_REG    ✓│ │ PROC_REG    ✓│  │
│  │ config_sync_server ✓│       │  │ config_sync  ✓│ │ config_sync  ✓│ │
│  │ gRPC List/Watch   ✓│       │  │ gRPC L/W    ✓│ │ gRPC L/W    ✓│  │
│  │ Status → K8s      ✓│       │  │ Status→K8s  ✓│ │ Status→K8s  ✗│  │
│  │ ACME              ✓│       │  │ ACME        ✓│ │ ACME        ✗│  │
│  └─────────────────────┘       │  └──────────────┘ └──────────────┘  │
│                                 │                                      │
│  Pod B (standby)                │  Gateway → 任意副本                  │
│  ┌─────────────────────┐       │                                      │
│  │ (完全待机)           │       │                                      │
│  └─────────────────────┘       │                                      │
│                                 │                                      │
│  Gateway → leader Service       │                                      │
└─────────────────────────────────┴──────────────────────────────────────┘
```

### 行为矩阵

| 行为 | `leader-only` | `all-serve` |
|------|---------------|-------------|
| K8s watchers 启动时机 | 仅 Leader | 所有副本 |
| gRPC 可用性 | Leader 就绪后 | 任意副本就绪后 |
| `set_config_sync_server(Some)` | Leader + CachesReady | CachesReady（任意副本） |
| Status 回写 K8s | Leader（`leader_handle` 守卫） | Leader（`leader_handle` 守卫） |
| ACME 证书自动化 | Leader（隐式） | Leader（显式守卫） |
| Leadership 丢失处理 | 退出 `run_main_flow`，清空一切，回到 `wait_until_leader` | 仅停 leader services，继续 gRPC |
| Gateway 连接目标 | `edgion-controller-leader` Service | `edgion-controller`（all pods）Service |
| Leader 切换时的 gRPC 中断 | 有（cache 重建 ~5-15s） | 无（其他副本继续服务） |
| K8s API server 负载 | 1x watcher 连接 | Nx watcher 连接 |
| 内存占用 | 仅 Leader 占用 cache 内存 | 所有副本占用 cache 内存 |

## Leader-Only 模式详解

### 生命周期

```
run_leader_only_lifecycle()
└── loop:
    ├── wait_until_leader()
    ├── run_main_flow()
    │   ├── start_event_watchers()      # 启动 Controller + Caches + Leader watcher
    │   ├── event loop                  # CachesReady → set gRPC server
    │   └── cleanup on exit             # stop ACME, clear registry
    └── match exit:
        ├── Shutdown → return
        └── LostLeadership → clear + loop back
```

### Gateway 配置

```toml
[gateway]
server_addr = "http://edgion-controller-leader.edgion-system.svc.cluster.local:50051"
```

### K8s Service 拓扑

| Service | Selector | 用途 |
|---------|----------|------|
| `edgion-controller` | `app: edgion-controller` | health check、metrics |
| `edgion-controller-leader` | `app: edgion-controller` + `edgion.io/leader: "true"` | Gateway gRPC、Admin API |

## All-Serve 模式详解

### 生命周期

```
run_all_serve_lifecycle()
└── loop:
    └── run_serving_flow()              # 不等 leadership，立即启动
        ├── start_event_watchers_all_serve()
        │   ├── Controller with leader_handle   # status 写入受守卫
        │   ├── Caches watcher (no ACME)        # ACME 移到 leader services
        │   ├── Leadership watcher (bidirectional)
        │   └── Reload watcher
        ├── event loop:
        │   ├── CachesReady → set gRPC server
        │   │   └── if is_leader → start_leader_services()
        │   ├── LeadershipAcquired
        │   │   └── if caches_ready → start_leader_services()
        │   ├── LeadershipLost
        │   │   └── stop_leader_services() (continue serving gRPC!)
        │   └── other events → exit
        └── cleanup
```

### 关键设计决策

#### Status 回写必须 Leader-Only

所有副本都通过 `ResourceProcessor` 计算 status（保证 gRPC cache 数据正确），
但只有 Leader 写回 K8s API。通过 `leader_handle.is_leader()` 守卫实现。

如果所有副本都写 status，会引发无限循环：
```
Pod-A 写 ResolvedRefs=True → Pod-B cache 没有 → 写 False → 覆盖 →
watch event 回弹 → 重新处理 → 循环...
```

#### ACME 必须 Leader-Only

ACME HTTP-01 challenge 要求单点请求 + 单点响应。
`start_acme_service()` 仅在 `start_leader_services()` 中调用。

#### Leader 切换时的 Status 对账

新 Leader 上任后调用 `PROCESSOR_REGISTRY.requeue_all()`，
触发全量 status 重算 + 回写，覆盖旧 Leader 可能遗留的 stale status。

#### server_id 跨副本差异

每个副本有独立的 `ConfigSyncServer`，server_id 不同。
Gateway 切换副本时检测到 server_id 变化，执行 full relist。
这是正确行为：确保副本间数据一致性。

### Leader Services

```rust
start_leader_services():
├── start_acme_service(client)
└── PROCESSOR_REGISTRY.requeue_all()    // 全量 status 对账

stop_leader_services():
└── stop_acme_service()
```

### Gateway 配置

```toml
[gateway]
server_addr = "http://edgion-controller.edgion-system.svc.cluster.local:50051"
```

### K8s Service 拓扑

| Service | Selector | 用途 |
|---------|----------|------|
| `edgion-controller` | `app: edgion-controller` | Gateway gRPC、health check、metrics |
| `edgion-controller-leader` | `app: edgion-controller` + `edgion.io/leader: "true"` | Admin API reload、ACME callback |

## /ready Endpoint 语义

`is_ready()` 检查 `PROCESSOR_REGISTRY.is_all_ready() && config_sync_server.is_some()`：

- **leader-only：** 非 Leader 返回 not ready → 不进入 Service endpoints
- **all-serve：** 所有就绪副本返回 ready → 都能接收 Gateway 流量

## 部署示例

### K8s Manifests

**Deployment** — 设置 `replicas: 2`

**Leader Service** — `examples/k8stest/kubernetes/controller/leader-service.yaml`

**RBAC** — Pod label patch 权限：
```yaml
rules:
- apiGroups: [""]
  resources: ["pods"]
  verbs: ["get", "patch"]
```

### 迁移路径

**单副本 → HA (leader-only)：**
1. Apply leader-service.yaml + RBAC
2. Gateway 指向 `edgion-controller-leader`
3. 设置 `replicas: 2`

**leader-only → all-serve：**
1. 设置 `ha_mode = "all-serve"`
2. Gateway 指向 `edgion-controller`（all-pods）
3. Rolling restart

**回滚 all-serve → leader-only：**
1. 设置 `ha_mode = "leader-only"`
2. Gateway 指向 `edgion-controller-leader`
3. Rolling restart
