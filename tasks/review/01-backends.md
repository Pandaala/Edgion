# 后端连接池与发现系统审查

**审查目录**: `src/core/gateway/backends/`
**审查文件**: 26 个 .rs 文件，约 5885 行代码

---

## H-1: 辅助 LB Store 僵尸条目不可回收 [高]

**文件**:
- `discovery/endpoint_slice/ep_slice_store.rs` 第 432-473 行
- `discovery/endpoint/endpoint_store.rs` 第 178-216 行

**问题描述**:

系统为每种 LB 算法维护独立的 Store（RoundRobin / Consistent / LeastConn / EWMA）。当请求首次使用某种 LB 策略时，通过 DCL 模式在对应 Store 中惰性创建 LB 实例。**但当路由的 LB 策略发生变更时（如从 ConsistentHash 切换到 RoundRobin），旧策略 Store 中的 LB 实例永远不会被清理**。

**泄漏链路**:
1. Route 配置 ConsistentHash → 首次请求 → CONSISTENT_STORE 中创建 `default/svc-a` 的 LB
2. Route 修改为 RoundRobin → 后续请求只走 ROUNDROBIN_STORE
3. CONSISTENT_STORE 中的 `default/svc-a` LB 永不被访问，但持续存在
4. 更糟的是，每次 EndpointSlice 数据变更时，`update_affected_lbs` 还会更新这个已无用的 LB

**内存影响**: 每个僵尸 LB 持有完整的 EndpointSlice 数据副本（`Arc<MultiEndpointSliceDiscovery>`）+ LB 算法状态。在大规模集群中，策略多次变更后，废弃的 LB 实例会持续占用内存。

**相关代码**:

```rust
// ep_slice_store.rs:323-355
pub fn update_lb_if_exists(&self, service_key: &str) {
    let _lock = self.creation_lock.lock().unwrap();
    let current = self.service_lbs.load();
    let lb = match current.get(service_key) {
        Some(lb) => lb.clone(),
        None => return,
    };
    match self.get_slices_for_service_internal(service_key) {
        Some(slices) if !slices.is_empty() => {
            // 只要 service 还有 endpoints，就会更新而非删除
            lb.update_slices(slices);
            lb.update_load_balancer().now_or_never();
        }
        _ => {
            // 只有 service 完全没有 endpoints 时才删除
            let mut new_map = (**current).clone();
            new_map.remove(service_key);
            self.service_lbs.store(Arc::new(new_map));
        }
    }
}
```

**建议修复方案**:

方案 A（推荐）：引入 LRU 淘汰或最后访问时间戳。在 `get_or_create_with_provider` 中记录 `last_access_time`，由定期清理任务清除超过 N 分钟未被访问的 LB 条目。

方案 B：在路由配置变更时，通知各 Store 清理不再使用的 LB 策略条目。可在路由 `partial_update` 时收集 `(service_key, lb_policy)` 对，对比并清理差异。

## L-3: HealthCheckManager 后台任务缺少优雅取消 [低]

**文件**: `health/check/manager.rs` 第 78-83, 96-221 行

**问题描述**:

`health_check_loop` 无限循环完全依赖 `JoinHandle::abort()` 进行强制取消，没有 `CancellationToken` 或 shutdown 信号。abort 在 .await 点取消任务，如果 HTTP 探测正在进行中，可能需要等待当前探测完成才会生效。

**建议修复**: 引入 `tokio_util::sync::CancellationToken`，在循环中用 `tokio::select!` 检查取消信号。

---

## L-4: HealthCheckManager 未限制并发任务数量 [低]

**文件**: `health/check/manager.rs` 第 26-27 行

**问题描述**:

`HashMap<String, JoinHandle<()>>` 存储后台健康检查任务，没有对并发任务数设置上限。在大规模集群中可能创建数百个并发任务，每个含 `reqwest::Client` 和定时器。

**建议修复**: 增加 `max_concurrent_checks` 配置项，或使用 `tokio::sync::Semaphore` 控制并发。

---

## 审查通过的子模块

| 子模块 | 审查结论 |
|--------|---------|
| ArcSwap clone-on-write 模式 | 正确，旧数据在 Guard 释放后自动回收 |
| Arc 循环引用 | 未发现，所有 Arc 为单向引用 |
| EndpointSlice 缓存清理 | 完整：`replace_data_only` 替换旧数据、`update_data_only` 正确处理 remove |
| 健康检查状态清理 | 完整：`unregister_service` + `states.retain()` 清理过期状态 |
| BackendTLSPolicyStore | 完整：`replace_all` 和 `update` 均正确清理 |
| HealthCheckConfigStore | 完整：三级配置独立管理，`full_set` 检测 stale entries |
| preload.rs | 仅启动时调用一次，临时数据全部释放 |
