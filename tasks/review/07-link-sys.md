# 链路系统 (LinkSys) 审查

**审查目录**: `src/core/gateway/link_sys/`
**审查文件**: 29 个 .rs 文件

---

## M-7: LinkSys dispatch 后台任务 JoinHandle 丢弃 [中]

**文件**: `runtime/store.rs` 第 72-81 行（full_set）, 第 100-109 行（partial_update）

**问题描述**:

`replace_all` 和 `update` 方法中，dispatch 操作通过 `handle.spawn()` 在后台异步执行，JoinHandle 被丢弃：

```rust
pub fn replace_all(&self, data: HashMap<String, LinkSys>) {
    let rt = tokio::runtime::Handle::try_current();
    if let Ok(handle) = rt {
        let data_clone = data.clone();
        handle.spawn(async move {
            dispatch_full_set(&data_clone).await;  // JoinHandle 被丢弃
        });
    }
    self.resources.store(Arc::new(Arc::new(data)));
}
```

更重要的是，`dispatch_full_set` 和 `dispatch_partial_update` 内部又会 `tokio::spawn` 多个子任务：
- Redis/Etcd/ES client 的 `init()` 任务（第 315-323 行等）
- 旧 client 的 `shutdown()` 任务（第 402-409 行等）

这些子任务的 JoinHandle 也全部被丢弃。如果 `init()` 永久阻塞（如连接到不存在的服务器），spawned task 会一直驻留。

**防护措施**（已有）: `config_mapping.rs` 中的 `MAX_CONNECT_TIMEOUT_MS` 限制了连接超时，避免 init 永久阻塞。但如果超时配置过大或被用户覆盖，仍存在风险。

**建议修复**: 对 init 任务设置全局超时上限（如 60s），使用 `tokio::time::timeout` 包裹 init 调用。

## 审查通过的子模块

| 子模块 | 审查结论 |
|--------|---------|
| Redis client 生命周期 | 正确：`from_config` → `init` → 存入 runtime，更新时 swap + shutdown 旧 client |
| Redis 连接池管理 | 正确：fred Pool 内建连接管理，`shutdown` 调用 `pool.quit()` 关闭所有连接 |
| Redis 健康检查 | 正确：AtomicBool 标记，on_reconnect/on_error 事件驱动更新 |
| Etcd client 生命周期 | 正确：与 Redis 相同模式 |
| ES bulk ingest | 正确：有 shutdown 信号、channel 关闭检测、缓冲 flush、重试上限 |
| ES client shutdown | 正确：watch channel 通知 → bulk loop 退出 → flush 残余数据 |
| Webhook Manager | 正确：upsert 时 abort 旧的健康检查任务，remove 时完整清理 |
| Webhook 健康检查 | 正确：JoinHandle 保存在 WebhookEntry 中，可被 abort |
| LocalFileWriter | 正确：文件 rotation 时关闭旧句柄（通过 Rust RAII），打开新文件 |
| 全局 runtime store 清理 | 正确：full_set 使用 `replace_all` 原子替换，旧 client 在后台 shutdown |
| DataSender trait | 正确：trait 提供 init/send/healthy/shutdown 完整生命周期 |
