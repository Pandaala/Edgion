# Gateway Runtime 审查

**审查目录**: `src/core/gateway/runtime/` + `src/bin/edgion_gateway.rs`
**审查文件**: 15 个 .rs 文件

---

## M-1: UDP listener JoinHandle 被丢弃，无生命周期管理 [中]

**文件**: `server/listener_builder.rs` 第 329-331 行

**问题描述**:

`tokio::spawn` 返回的 `JoinHandle` 被直接丢弃：

```rust
tokio::spawn(async move {
    edgion_udp.serve().await;
});
```

问题：
1. **无法取消任务**：没有 `CancellationToken` 或 shutdown 信号，无法实现优雅关停
2. **Panic 不可感知**：`edgion_udp.serve()` 如果 panic，没有 JoinHandle 来捕获和上报
3. **资源无法主动释放**：`Arc<EdgionUdp>`（含 UdpSocket、路由管理器引用）只有在任务自然结束或 Tokio runtime 强制关闭时才释放

**对比**: HTTP/TCP/TLS listener 通过 Pingora 的 `server.add_service()` 获得了框架级的生命周期管理，但 UDP listener 绕过了这一机制。

**建议修复**:

```rust
let cancel_token = CancellationToken::new();
let token_clone = cancel_token.clone();
let handle = tokio::spawn(async move {
    tokio::select! {
        _ = token_clone.cancelled() => {
            tracing::info!("UDP listener shutting down");
        }
        _ = edgion_udp.serve() => {}
    }
});
// 将 handle 和 cancel_token 存储在可管理的集合中
```

---

## L-8: add_header 方法签名错误 [低]

**文件**: `server/server_header.rs` 第 27-29 行

**问题描述**:

方法接受 `mut self`（按值获取所有权），而不是 `&mut self`。调用 `opts.add_header("X-Custom", "value")` 会消费 `opts`，修改后立即 drop，修改完全丢失。

```rust
pub fn add_header<S: Into<String>>(mut self, key: S, value: S) {
    self.headers.insert(key.into(), value.into());
}
```

当前代码中未找到对 `add_header` 的实际调用，所以暂无运行时影响，但 API 是有问题的。

**建议修复**:

```rust
// 方案 A：改为 &mut self
pub fn add_header<S: Into<String>>(&mut self, key: S, value: S) {
    self.headers.insert(key.into(), value.into());
}

// 方案 B：Builder 模式
pub fn add_header<S: Into<String>>(mut self, key: S, value: S) -> Self {
    self.headers.insert(key.into(), value.into());
    self
}
```

---

## 审查通过的子模块

| 文件 | 审查结论 |
|------|---------|
| `handler.rs` | RwLock 使用正确：写锁在 block scope 内释放，无 `.await` 点跨越 |
| `store/config.rs` | ArcSwap 使用正确：`full_set` 完整替换、`update_gateway` clone-and-modify |
| `store/gateway.rs` | HashMap 有 `clear()` 和 `remove()` 方法，全局 LazyLock 生命周期与进程一致 |
| `store/port_gateway_info.rs` | ArcSwap `rebuild` 完整替换，`get()` 返回的 Arc 在调用方用完即释放 |
| `matching/route.rs` | 纯函数，ArcSwap Guard 在函数返回时自动释放 |
| `matching/tls.rs` | `rebuild_from_gateways` 整体替换旧数据，旧 Arc 自动回收 |
| `gateway_info.rs` | 纯数据结构，无 Arc/锁/Channel |
| `bin/edgion_gateway.rs` | 简单入口，`cli.run()` 阻塞直到退出 |
