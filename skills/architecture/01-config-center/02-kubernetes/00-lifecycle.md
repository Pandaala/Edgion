# KubernetesCenter 生命周期与 Leader Election

## 启动流程

`KubernetesCenter::start()` 是 Kubernetes 模式的入口。根据 `ha_mode` 配置分发到不同生命周期：

```
start()
├── 1. Client::try_default()            # 创建 K8s 客户端
├── 2. LeaderElection::new()            # 初始化 Leader Election
├── 3. leader_election.preflight_check()  # 前置校验（失败直接 Err 退出）
│      ├── ensure_lease_exists()        #   确认 Lease 可读/创建
│      └── ensure_pod_label_patch_ready() # 确认 Pod label 可 patch（权限检查）
├── 4. tokio::spawn(le.run())           # 前置校验通过后才启动后台选举循环
└── 5. match ha_mode:
    ├── LeaderOnly → run_leader_only_lifecycle()
    └── AllServe   → run_all_serve_lifecycle()
```

### Leader-Only 生命周期（默认）

```
run_leader_only_lifecycle()
└── loop:
    ├── wait_until_leader()             # 阻塞直到成为 Leader
    ├── run_main_flow()                 # 运行所有 watchers + gRPC
    └── match exit:
        ├── Shutdown → return
        └── LostLeadership → clear + loop back
```

### All-Serve 生命周期

详见 [01-ha-mode.md](01-ha-mode.md)。

## Leader Election（Lease-based）

使用 Kubernetes Lease 对象实现分布式 Leader Election：

```
LeaderElection
├── client: kube::Client
├── config: LeaderElectionConfig
│   ├── lease_name: "edgion-controller-leader"
│   ├── lease_namespace: "edgion-system"
│   ├── pod_namespace: $POD_NAMESPACE (for pod label patch)
│   ├── identity: $POD_NAME (from Downward API)
│   ├── lease_duration_secs: 15
│   ├── renew_period_secs: 10
│   └── retry_period_secs: 2
└── is_leader: Arc<AtomicBool>
```

### 选举循环

```
run():                                  # 由 tokio::spawn 后台运行；前置校验已在 start() 中完成
└── loop:
    ├── sleep(renew/retry interval)
    ├── try_acquire_or_renew()
    │   ├── 读取当前 Lease
    │   ├── 检查 holder_identity 和过期时间
    │   └── Server-Side Apply 更新 Lease（使用 Ordering::Release store）
    └── match result:
        ├── Ok(true)  → swap is_leader=true (Release), update pod label
        ├── Ok(false) → swap is_leader=false (Release), update pod label
        └── Err       → assume lost, swap is_leader=false (Release), update pod label
```

> **内存序说明**：`is_leader` 的写操作（`swap`/`store`）使用 `Ordering::Release`，  
> 读操作（`load`）使用 `Ordering::Acquire`，保证跨线程的 happens-before 语义，  
> 在 ARM/M1 等弱内存序架构上确保 status write 守卫的正确性。

### Pod Leader Label

每次 leadership 状态变化时，Controller 更新当前 Pod 的 label：

```yaml
metadata:
  labels:
    edgion.io/leader: "true"   # or "false"
```

这使得 `edgion-controller-leader` Service 可以通过 selector 只路由到 leader Pod。
Label 更新是 fire-and-forget，失败不阻塞选举循环。

### LeaderHandle

`LeaderHandle` 是一个轻量级的 clone-able 句柄，内部持有 `Arc<AtomicBool>`：

```rust
pub struct LeaderHandle {
    is_leader: Arc<AtomicBool>,
}

impl LeaderHandle {
    pub fn is_leader(&self) -> bool;
    pub async fn wait_until_leader(&self);
    pub async fn wait_until_leader_with_shutdown(&self, shutdown: ShutdownSignal) -> bool;
}
```

在 ResourceController 中用于 status 写入守卫。

## Event-Driven Main Flow

`run_main_flow()` 使用事件驱动架构，通过 `mpsc::channel` 接收生命周期事件：

```rust
enum LifecycleEvent {
    CachesReady,            // PROCESSOR_REGISTRY 所有处理器就绪
    CachesTimeout,          // 超时未就绪
    LeadershipLost,         // 失去 leadership
    LeadershipAcquired,     // 获得 leadership（all-serve 模式）
    ControllerExit(reason), // KubernetesController 退出
    ReloadRequested,        // Admin API 触发 reload
}
```

### Watcher Tasks

`start_event_watchers()` 启动四个后台任务：

| Task | 职责 | 发送事件 |
|------|------|---------|
| Controller | 运行 `KubernetesController.run()` | `ControllerExit` |
| Caches | 等待 `PROCESSOR_REGISTRY` 就绪 | `CachesReady` / `CachesTimeout` |
| Leader | 监控 leadership 状态变化（100ms 轮询） | `LeadershipLost` / `LeadershipAcquired` |
| Reload | 监听 Admin API reload 信号 | `ReloadRequested` |

> **Leader Watcher 初始状态广播**（all-serve 模式）：  
> Leader watcher 启动时会**立即检查当前状态**，若已是 leader 则立刻发送一次 `LeadershipAcquired`，  
> 而不等待下一次 100ms 轮询触发。这保证了副本重启后（重启前已是 leader）能立即恢复 leader 服务，  
> 不会错过成为 leader 的初始状态。

### 重试与 Backoff

`run_main_flow` 内置指数退避重试：

- 连续失败计数：每次 controller 异常退出 +1
- 稳定运行 5 分钟后重置计数
- 退避时间：`2^min(failures, 6)` 秒（1s, 2s, 4s, ... 64s）
- 超过 10 次连续失败后放弃

## 配置参考

```toml
[conf_center]
type = "kubernetes"
gateway_class = "edgion"
controller_name = "edgion.io/gateway-controller"
ha_mode = "leader-only"          # or "all-serve"

[conf_center.leader_election]
lease_name = "edgion-controller-leader"
lease_namespace = "edgion-system"
lease_duration_secs = 15
renew_period_secs = 10
retry_period_secs = 2
```

## 关键文件

- `conf_center/kubernetes/center.rs` — 生命周期核心
- `conf_center/kubernetes/leader_election.rs` — Leader Election 实现
- `conf_center/kubernetes/config.rs` — 配置结构体
