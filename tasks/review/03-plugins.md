# 插件系统审查

**审查目录**: `src/core/gateway/plugins/`
**审查范围**: HTTP 插件、Stream 插件、PluginRuntime、ConditionalFilter、全局 HTTP Client

---

## M-2: ULogBuffer 无大小限制 [中]

**文件**: `plugins/runtime/log.rs` 第 67-85 行

**问题描述**:

`LogBuffer`（固定缓冲区）有 100 字节容量 + 20 条目上限的双重限制，设计良好。但 `ULogBuffer` 没有任何大小限制，`push` 方法无条件追加数据到堆分配的 `String` 和 `Vec<usize>`。

```rust
pub struct ULogBuffer {
    buffer: String,           // 无限增长
    positions: Vec<usize>,    // 无限增长
}

impl ULogBuffer {
    fn new() -> Self {
        Self {
            buffer: String::with_capacity(256),
            positions: Vec::with_capacity(32),
        }
    }

    fn push(&mut self, log: &str) {
        self.buffer.push_str(log);  // 无条件追加
        self.positions.push(self.buffer.len());
    }
}
```

虽然 `ULogBuffer` 标注为 debug/trace 用途，且生命周期为 per-request，但如果 debug 插件在高流量下对每个请求生成大量日志，单个请求的 `ULogBuffer` 可能增长到很大。

**建议修复**:

```rust
const ULOG_MAX_BUFFER: usize = 65536;
const ULOG_MAX_ENTRIES: usize = 1000;

fn push(&mut self, log: &str) -> bool {
    if self.positions.len() >= ULOG_MAX_ENTRIES
        || self.buffer.len() + log.len() > ULOG_MAX_BUFFER {
        return false;
    }
    self.buffer.push_str(log);
    self.positions.push(self.buffer.len());
    true
}
```

---

## M-3: OpenidConnect 缓存粗暴清理策略 [中]

**文件**: `plugins/http/openid_connect/openid_impl.rs` 第 1155-1157, 1566-1568, 1587-1589 行

**问题描述**:

OpenidConnect 的多个内部缓存使用"达到 4096 后全清"的策略：

```rust
if cache.len() > 4096 {
    cache.clear();
}
```

此模式出现在 `access_token_cache`、`refresh_singleflight_locks` 和 `refresh_singleflight_results` 中。

问题：
1. **Thundering Herd**: 缓存达到 4096 后全部清空，缓存命中率瞬间跌为零
2. **OIDC Provider 压力**: 清空后所有并发请求同时去刷新 token
3. **introspection_cache**: 使用 `retain` 清理过期条目，但没有硬上限，理论上在 TTL 内可无限增长

**建议修复**:
1. 将 `cache.clear()` 改为 LRU 淘汰（保留最近使用的一半条目）
2. 为 `introspection_cache` 添加 4096 硬上限
3. 或使用 `mini-moka` / `quick-cache` 等带 TTL + 容量限制的缓存库

## L-6: ExtensionRef body filter 缺少 panic 保护的 pop [低]

**文件**: `plugins/runtime/gateway_api_filters/extension_ref.rs` 第 201-234 行

**问题描述**:

在 `run_extension_body` 中，如果内部插件的 `run_upstream_response_body_filter` panic，`session.pop_plugin_ref()` 不会被调用，导致 `plugin_ref_stack` 状态不一致。

同步的 `run_extension` 和异步的 `run_extension_async` 都通过 `Self::finish` 正确处理了 pop，但 `run_extension_body` 直接在末尾 pop，没有使用 `finish` 模式。

**建议修复**: 使用 RAII guard 模式确保 pop 一定执行。

---

## 审查通过的子模块

| 子模块 | 审查结论 |
|--------|---------|
| PluginRuntime 生命周期 | 正确：配置热更新时旧实例通过 Arc 引用计数自动回收 |
| ctx_map / ctx_var | 安全：per-request 生命周期，不跨请求持久化 |
| LogBuffer（固定缓冲区） | 设计良好：100 字节容量 + 20 条目限制 |
| ExtensionRef 循环检测 | 完善：`has_plugin_ref` + `max_depth` 双重保护 |
| ConditionalFilter | 安全：独占所有权，无循环引用 |
| Box<dyn Plugin> 动态分派 | 安全：标准 trait object 管理 |
| 插件间数据传递 | 正确使用 `std::mem::take` 所有权转移 |
| StreamPlugin | 安全：ArcSwap 使用正确 |
| 全局 HTTP Client | 安全：OnceLock + 连接池限制 + 超时设置 |
| RequestMirrorPlugin | 低风险：每实例独立 Client，配置更新不频繁 |
