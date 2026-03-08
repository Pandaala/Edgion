# 代码质量与跨模块问题

---

## Workqueue 设计审查 [审查通过]

**文件**: `controller/conf_mgr/sync_runtime/workqueue.rs`

Workqueue 设计健壮，具备完整的防泄漏机制：

1. **有界通道**: `mpsc::channel(config.capacity)` 限制队列大小（默认 1000）
2. **去重**: `DashSet<String>` 防止同一 key 重复入队
3. **退避重试上限**: `max_retries: 5` 限制最大重试次数，超过后放弃
4. **触发链循环检测**: `TriggerChain` + `max_trigger_cycles: 5` + `max_trigger_depth: 20` 防止无限级联
5. **Delay Queue**: 延迟队列有 dedup 检查（`pending` + `scheduled` 两层），防止重复调度
6. **SmallVec 优化**: 触发链使用 `SmallVec<[TriggerSource; 4]>` 避免小链的堆分配

---

## Controller K8s Center 生命周期管理 [审查通过]

**文件**: `controller/conf_mgr/conf_center/kubernetes/center.rs`

Controller 的生命周期管理设计完善：
- 所有 `tokio::spawn` 的 JoinHandle 都保存在 `WatcherHandles` 中
- `abort_and_wait()` 确保所有任务被取消
- `PROCESSOR_REGISTRY.clear_registry()` 在 leadership 丢失和 shutdown 时清理全局注册表
- `set_config_sync_server(None)` 清理 gRPC server 引用
- `stop_leader_services()` 停止 ACME 等 leader-only 服务

---

## 全局 DashMap 使用审查

项目使用 `DashMap` 作为并发 HashMap 的多处需要关注：

| 位置 | 类型 | 清理机制 | 风险 |
|------|------|---------|------|
| `ewma/metrics.rs` | `DashMap<SocketAddr, AtomicU64>` | 需外部调用 `remove()` | **高**（见 H-2） |
| `leastconn/counter.rs` | `DashMap<SocketAddr, AtomicUsize>` | BackendCleaner 清理 draining | **高**（见 H-2） |
| `workqueue.rs` pending | `DashSet<String>` | dequeue 时自动 remove | 安全 |
| `workqueue.rs` scheduled | `DashSet<String>` | delay loop 出队时 remove | 安全 |

---

## EventStore 循环缓冲审查 [审查通过]

**文件**: `controller/conf_sync/cache_server/store.rs`

EventStore 设计正确：
- 固定容量循环缓冲（默认 200，最小 10）
- `clear()` 完整重置所有状态
- Stale event guard 防止过期事件覆盖新状态（`sync_version` 单调递增检查）
- `snapshot_owned()` 返回 clone 数据，不影响内部状态
