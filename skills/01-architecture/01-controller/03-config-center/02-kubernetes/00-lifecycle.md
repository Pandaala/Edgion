---
name: kubernetes-lifecycle
description: KubernetesCenter 启动流程与 Leader 选举：Lease-based 选举、事件驱动主循环、Watcher 任务管理、ConfigSyncServer 发布时机。
---

# Kubernetes 配置中心 — 生命周期

## 启动流程

`KubernetesCenter::start()` 是 Kubernetes 模式的入口，根据 `ha_mode` 配置分发到不同生命周期：

```
start(shutdown_handle)
├── 1. Client::try_default()              # 创建 K8s 客户端（集群内 ServiceAccount 或 kubeconfig）
├── 2. LeaderElection::new(client, config) # 初始化 Leader Election
├── 3. leader_election.preflight_check()   # 前置校验（失败直接返回 Err，进程退出）
│      ├── ensure_lease_exists()           #   确认 Lease 对象可读/可创建
│      └── ensure_pod_label_patch_ready()  #   确认 Pod label 可 patch（RBAC 权限检查）
├── 4. tokio::spawn(le.run())             # 校验通过后才启动后台选举循环
└── 5. match ha_mode:
    ├── LeaderOnly → run_leader_only_lifecycle(client, leader_handle, shutdown_handle)
    └── AllServe   → run_all_serve_lifecycle(client, leader_handle, shutdown_handle)
```

## Leader-Only 生命周期（默认）

```
run_leader_only_lifecycle()
└── loop:
    ├── wait_until_leader()               # 阻塞直到成为 Leader
    ├── run_main_flow()                   # 运行所有 watchers + gRPC
    │   ├── start_event_watchers()
    │   ├── event loop (CachesReady → set gRPC server)
    │   └── cleanup on exit (stop ACME, clear registry)
    └── match exit:
        ├── Shutdown → return
        └── LostLeadership → set_config_sync_server(None) + clear + loop back
```

非 Leader 副本完全待机，不启动任何 watcher，不消耗 K8s API 连接或内存。
失去 leadership 后清空所有状态并回到等待循环。

## All-Serve 生命周期

```
run_all_serve_lifecycle()
└── loop:
    └── run_serving_flow()                # 不等 leadership，立即启动
        ├── start_event_watchers_all_serve()
        │   ├── Controller（with leader_handle）  # status 写入受守卫
        │   ├── Caches watcher（no ACME）         # ACME 移到 leader services
        │   ├── Leadership watcher（bidirectional）
        │   └── Reload watcher
        ├── event loop:
        │   ├── CachesReady → set gRPC server
        │   │   └── if is_leader → start_leader_services()
        │   ├── LeadershipAcquired
        │   │   └── if caches_ready → start_leader_services()
        │   ├── LeadershipLost
        │   │   └── stop_leader_services()（继续 gRPC 服务！）
        │   └── ControllerExit / ReloadRequested → exit
        └── cleanup: set_config_sync_server(None), clear registry
```

所有副本独立运行 K8s watchers 和 gRPC 服务，Leader 额外承担 status 回写和 ACME。
详细对比见 [01-ha-mode.md](01-ha-mode.md)。

## Leader Election（Lease-based）

使用 Kubernetes Lease 对象实现分布式 Leader Election：

```
LeaderElection
├── client: kube::Client
├── config: LeaderElectionConfig
│   ├── lease_name: "edgion-controller-leader"
│   ├── lease_namespace: "edgion-system"
│   ├── pod_namespace: $POD_NAMESPACE（Downward API）
│   ├── identity: $POD_NAME（Downward API）
│   ├── lease_duration_secs: 15
│   ├── renew_period_secs: 10
│   └── retry_period_secs: 2
└── is_leader: Arc<AtomicBool>
```

### 选举循环

```
run():                                    # tokio::spawn 后台运行
└── loop:
    ├── sleep(renew/retry interval)
    ├── try_acquire_or_renew()
    │   ├── 读取当前 Lease
    │   ├── 检查 holder_identity 和过期时间
    │   └── Server-Side Apply 更新 Lease
    └── match result:
        ├── Ok(true)  → store is_leader=true (Release), update pod label
        ├── Ok(false) → store is_leader=false (Release), update pod label
        └── Err       → assume lost, store is_leader=false (Release), update pod label
```

**内存序说明**：`is_leader` 的写操作使用 `Ordering::Release`，读操作使用 `Ordering::Acquire`，
保证跨线程的 happens-before 语义，在 ARM/M1 等弱内存序架构上确保 status write 守卫的正确性。

### Pod Leader Label

每次 leadership 状态变化时更新 Pod label：

```yaml
metadata:
  labels:
    edgion.io/leader: "true"   # or "false"
```

`edgion-controller-leader` Service 通过此 selector 只路由到 leader Pod。
Label 更新是 fire-and-forget，失败不阻塞选举循环。

### LeaderHandle

轻量级 clone-able 句柄，内部持有 `Arc<AtomicBool>`：

```rust
pub struct LeaderHandle {
    is_leader: Arc<AtomicBool>,
}

impl LeaderHandle {
    pub fn is_leader(&self) -> bool;                          // Acquire load
    pub async fn wait_until_leader(&self);                    // 阻塞等待
    pub async fn wait_until_leader_with_shutdown(&self, ...) -> bool;
}
```

传递给 `ResourceController`，用于 `persist_k8s_status()` 的 leader 守卫。

## 事件驱动主流程（Event-Driven Main Flow）

`run_main_flow()` 使用事件驱动架构，通过 `mpsc::channel<LifecycleEvent>` 接收生命周期事件：

```rust
enum LifecycleEvent {
    CachesReady,                  // 所有 phased processors 已注册 + 所有 sync kinds 已就绪
    CachesTimeout,                // 超时未就绪
    LeadershipLost,               // 失去 leadership
    LeadershipAcquired,           // 获得 leadership（all-serve 模式专用）
    ControllerExit(reason),       // KubernetesController 退出（正常/410 Gone/错误）
    ReloadRequested,              // Admin API 触发 reload
}
```

主循环是一个简单的 `match event_rx.recv().await`，没有复杂的 `select!`，事件处理逻辑清晰：

| 事件 | leader-only 处理 | all-serve 处理 |
|------|-----------------|----------------|
| `CachesReady` | 创建 ConfigSyncServer + 注册 WatchObj | 同左，若已是 leader 则额外 start_leader_services |
| `CachesTimeout` | 同上（降级处理） | 同上 |
| `LeadershipLost` | 退出 run_main_flow | stop_leader_services，继续 gRPC 服务 |
| `LeadershipAcquired` | 不使用 | 若 caches_ready 则 start_leader_services |
| `ControllerExit` | 退出循环，触发重试/重建 | 同左 |
| `ReloadRequested` | 退出循环，触发 reload | 同左 |

## Watcher 任务

`start_event_watchers()` 启动四个后台 tokio 任务：

| Task | 职责 | 发送事件 |
|------|------|---------|
| **Controller** | 运行 `KubernetesController.run()`（spawn 所有 ResourceController） | `ControllerExit(reason)` |
| **Caches** | 等待所有 phased processors 注册完成 + 所有同步到 Gateway 的 kinds ready | `CachesReady` / `CachesTimeout` |
| **Leader** | 监控 leadership 状态变化（100ms 轮询 `AtomicBool`） | `LeadershipLost` / `LeadershipAcquired` |
| **Reload** | 监听 Admin API reload 信号（`mpsc::Receiver`） | `ReloadRequested` |

**Leader Watcher 初始状态广播**（all-serve 模式）：
Leader watcher 启动时立即检查当前状态，若已是 leader 则立刻发送一次 `LeadershipAcquired`，
而不等待下一次 100ms 轮询。这保证副本重启后（重启前已是 leader）能立即恢复 leader 服务。

`WatcherHandles` 结构体持有所有四个任务的 `JoinHandle`，清理时通过 `abort_and_wait()` 统一终止。

## ConfigSyncServer 发布时机

ConfigSyncServer 的发布（`set_config_sync_server(Some(...))`）必须满足严格的时序条件。

### 问题场景

Kubernetes 分阶段初始化中，Phase 2 资源（HTTPRoute、GRPCRoute、EdgionTls、EdgionPlugins 等）
在 Phase 1 之后才注册到 `PROCESSOR_REGISTRY`。如果 Caches watcher 只检查
"当前 registry 里没有 not_ready kinds"就发送 `CachesReady`，会导致 ConfigSyncServer
只包含 Phase 1 的 WatchObj 集合。Gateway 对 Phase 2 kinds 做 `List(kind)` 会收到 `Unknown kind`。

### 正确规则

1. **等待所有 phased processors 注册完成**（Phase 1 + Phase 2 全部 spawn 完毕）
2. **等待所有会同步到 Gateway 的 kinds 标记 ready**（init 阶段完成）
3. **执行 `PROCESSOR_REGISTRY.all_watch_objs(no_sync_kinds)` 并发布 `CachesReady`**

### 排障提示

如果 Gateway 日志反复出现 `Failed to list resources: Unknown kind: HTTPRoute/GRPCRoute/EdgionTls/...`，
但已有流量仍可用，优先检查是否 controller restart/reload 后只发布了部分 WatchObj。

## 重试与 Backoff

`run_main_flow` 内置指数退避重试机制：

| 参数 | 值 |
|------|-----|
| 退避公式 | `2^min(failures, 6)` 秒（1s → 2s → 4s → 8s → 16s → 32s → 64s） |
| 最大连续失败 | 10 次后放弃（进程退出） |
| 稳定运行重置 | 连续运行 5 分钟后重置失败计数 |

每次 controller 异常退出或 410 Gone 触发重建时，连续失败计数 +1。
如果一次迭代稳定运行超过 5 分钟，计数归零。

## 配置参考

```toml
[conf_center]
type = "kubernetes"
gateway_class = "edgion"
controller_name = "edgion.io/gateway-controller"
ha_mode = "leader-only"            # or "all-serve"

[conf_center.leader_election]
lease_name = "edgion-controller-leader"
lease_namespace = "edgion-system"
lease_duration_secs = 15
renew_period_secs = 10
retry_period_secs = 2
```

## 关键文件

- `src/core/controller/conf_mgr/conf_center/kubernetes/center.rs` — 生命周期核心（`start`, `run_main_flow`, `run_serving_flow`）
- `src/core/controller/conf_mgr/conf_center/kubernetes/leader_election.rs` — Leader Election 实现
- `src/core/controller/conf_mgr/conf_center/kubernetes/controller.rs` — `KubernetesController`（spawn 资源控制器）
- `src/core/controller/conf_mgr/conf_center/kubernetes/config.rs` — `KubernetesConfig`, `HaMode`
- `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs` — `ResourceController<K>`
