# 修复建议与优先级

---

## 第一优先级：真实内存泄漏风险

### 1. EWMA/LeastConn 全局 DashMap 清理 (H-2)

**影响范围**: 所有使用 EWMA 或 LeastConn 策略的场景
**复现条件**: K8s Pod 滚动更新、HPA 扩缩容等导致后端 IP 变化
**预估影响**: 每个过期条目约 40-80 字节，频繁滚动更新的集群数周内可能积累数万条目

**修复方案**:

```rust
// 在 EndpointSlice LB 更新流程中，比较新旧后端列表，清理被移除的后端：

// 方案 1：在 LoadBalancer update 时返回被移除的地址
pub fn update_and_get_removed(&mut self, backends: &Backends) -> Vec<SocketAddr> {
    let new_addrs: HashSet<_> = backends.iter().map(|b| b.addr.clone()).collect();
    let old_addrs: HashSet<_> = self.current_backends().map(|b| b.addr.clone()).collect();
    let removed: Vec<_> = old_addrs.difference(&new_addrs).cloned().collect();
    self.update(backends);
    removed
}

// 调用方：
for addr in removed {
    crate::core::gateway::lb::ewma::metrics::remove(&addr);
    crate::core::gateway::lb::leastconn::counter::remove(&addr);
    crate::core::gateway::lb::leastconn::backend_state::remove(&addr);
}
```

### 2. 辅助 LB Store 僵尸条目清理 (H-1)

**影响范围**: 路由 LB 策略变更的场景
**复现条件**: 修改路由的 LB 策略（如从 ConsistentHash 切换到 RoundRobin）
**预估影响**: 每个僵尸 LB 持有完整的 EndpointSlice 数据副本

**修复方案**: 引入 LRU/最后访问时间戳，定期清理未使用的 LB 实例。

---

## 第二优先级：性能与稳定性

### 3. HTTP Header/Query 正则预编译 (M-4)

**修复难度**: 低（gRPC 已有现成方案）
**性能提升**: 消除每请求的 Regex 编译，在使用正则 Header 匹配的场景下可能有显著的 CPU 和内存分配优化

### 4. OpenidConnect 缓存策略改进 (M-3)

**修复方案**: 将 `cache.clear()` 改为 LRU 淘汰（保留最近使用的一半条目），或引入 `mini-moka` 缓存库

### 5. ULogBuffer 大小限制 (M-2)

**修复难度**: 很低
**修复方案**: 添加 `ULOG_MAX_BUFFER = 65536` 和 `ULOG_MAX_ENTRIES = 1000` 限制

---

## 第三优先级：代码质量与一致性

### 7. 后台任务生命周期管理 (M-1, M-5, M-6, M-7)

**统一方案**: 引入 `tokio_util::sync::CancellationToken`，为所有长生命周期的 `tokio::spawn` 任务提供优雅取消能力。优先处理：
1. UDP listener (M-1)
2. LinkSys dispatch tasks (M-7)

CacheData 和 Watch stream 的任务 (M-5, M-6) 由于生命周期与进程一致，优先级较低。

### 8. 其他低优先级修复

- L-8: `add_header` 方法签名修复
- L-7: gRPC full_set 减少不必要的 Clone
- L-3/L-4: 健康检查任务优雅取消和并发限制
- L-6: ExtensionRef body filter RAII guard

---

## 架构建议

### 全局 DashMap 管理策略

项目中有多处使用全局 `DashMap` 作为运行时状态存储。建议制定统一的清理策略：

1. **显式清理接口**: 每个全局 DashMap 都应提供 `cleanup_stale_entries()` 方法
2. **定期清理任务**: 考虑在 Gateway 的后台服务中增加一个统一的清理任务，定期扫描各全局 DashMap
3. **度量监控**: 为关键的全局 DashMap 暴露 `len()` 到 Prometheus metrics，方便监控增长趋势

### tokio::spawn 管理规范

建议为所有长生命周期的 `tokio::spawn` 建立统一模式：
1. 保存 JoinHandle（或使用 `JoinSet`）
2. 引入 CancellationToken 用于优雅关停
3. 为可能阻塞的异步操作（如网络连接）设置超时
