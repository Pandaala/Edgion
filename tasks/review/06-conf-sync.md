# 配置同步系统审查

**审查目录**:
- `src/core/common/conf_sync/`
- `src/core/gateway/conf_sync/`
- `src/core/controller/conf_sync/`

---

## M-5: CacheData 后台 tokio::spawn 任务无法取消 [中]

**文件**: `gateway/conf_sync/cache_client/cache_data.rs` 第 103-117 行

**问题描述**:

`set_conf_processor` 方法通过 `tokio::spawn` 启动一个每 100ms 处理压缩事件的后台任务。该任务的 JoinHandle 被直接丢弃，且任务通过 `Arc<RwLock<CacheData<T>>>` 持有对 CacheData 的引用。

```rust
pub(crate) fn set_conf_processor(
    &mut self,
    processor: Box<dyn ConfHandler<T> + Send + Sync>,
    cache_data: Arc<RwLock<CacheData<T>>>,
) {
    self.handler = Some(ConfHandlerData::new(processor));
    let cache_data_clone = cache_data.clone();

    tokio::spawn(async move {
        let mut interval = interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if !Self::process_compressed_events(&cache_data_clone) {
                break;
            }
        }
    });
}
```

问题：
1. JoinHandle 被丢弃，无法取消任务
2. 后台任务通过 `cache_data_clone` 持有 `Arc<RwLock<CacheData>>` 引用，阻止 CacheData 被释放
3. 任务只在 handler 被移除（返回 false）时退出，但没有代码路径会将 handler 设置为 None
4. 每种资源类型一个此类任务（共约 18 个），进程生命周期内永不退出

**实际影响**: 由于 ConfigClient 是全局单例，这些任务的生命周期与进程一致，不会造成实际的内存泄漏。但设计上缺少优雅关停能力。

**建议修复**: 存储 JoinHandle，在需要时（如测试场景、热重载）可以取消。或使用 `CancellationToken`。

---

## M-6: gRPC Watch stream 后台任务无法取消 [中]

**文件**: `gateway/conf_sync/cache_client/event_dispatch.rs` 第 130-364 行

**问题描述**:

`start_watch` 方法通过 `tokio::spawn` 启动一个包含无限循环的后台任务，负责 List → Watch → 断连重连。JoinHandle 被直接丢弃。

```rust
pub async fn start_watch(&self) -> Result<(), tonic::Status> {
    let grpc_client = self.grpc_client.clone();
    let cache_data = self.cache_data.clone();
    // ...
    tokio::spawn(async move {
        // 外层循环：List 操作
        loop {
            // 内层循环：Watch stream 消息处理
            // ...
        }
    });
    Ok(())
}
```

问题与 M-5 类似：
1. JoinHandle 丢弃，无法取消
2. 每种资源类型一个 watch 任务（约 18 个），永不退出
3. 断连重连时旧的 gRPC stream 被正确释放（break 后旧 stream drop），这部分是正确的

**实际影响**: 与 M-5 相同，生命周期与进程一致，无实际泄漏。但缺少优雅关停能力。

**建议修复**: 保存 JoinHandle，引入 CancellationToken 用于优雅关停。

---

## 审查通过的子模块

| 子模块 | 审查结论 |
|--------|---------|
| ConfigSyncClient | 正确：gRPC channel `connect_lazy` 延迟连接，断连时资源释放 |
| ClientCache 缓存管理 | 正确：`reset()` 完整替换 HashMap，`apply_change` 正确处理 insert/remove |
| WatchServerMeta | 正确：stream 断连时正确 break，backoff 有上限 |
| ConfigSyncServer | 正确：watch objects 通过 RwLock 管理，`clear_all` 清理完整 |
| EventStore (Controller) | 正确：循环缓冲有固定容量，`clear()` 完整重置 |
| ClientRegistry | 正确：register/unregister 完整生命周期 |
| CompressEvent | 正确：`clear()` 在 full_set 和 partial_update 后调用 |
| ConfigClient | 正确：exhaustive match 保证编译时完整性 |
| ConfigSyncServerProvider | 正确：`reset_for_relink` 完整清理状态 |
