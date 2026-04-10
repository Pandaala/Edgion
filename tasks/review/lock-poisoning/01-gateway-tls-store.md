# Gateway TLS Store — 锁中毒 Review

## 模块概述

`src/core/gateway/tls/store/tls_store.rs` 实现了 `TlsStore`，即整个网关的 TLS 证书存储。通过 `GLOBAL_TLS_STORE: LazyLock<Arc<TlsStore>>`（第 461 行）提供进程唯一的全局实例。

该模块的职责：
- 存储所有 TLS 证书（`tls_data: RwLock<HashMap<String, TlsEntry>>`）
- 通过 `ConfHandler<EdgionTls>` trait 接收来自 Controller 的全量/增量配置同步
- 在写入时重建 `TlsCertMatcher`（基于 `ArcSwap`，无锁读）供 TLS 握手匹配使用
- 提供 API 端点查询证书统计（`get_cert_stats`、`entry_count`）和诊断数据（`list_all`、`get_invalid_certs`）

**关键性**：P0。`TlsStore` 是 TLS 证书数据的唯一来源，写路径触发 matcher 重建直接影响所有 HTTPS/TLS 握手。一旦锁中毒导致读路径不可用，所有 TLS 相关的 API 端点和诊断功能将级联 panic。

---

## 锁使用清单

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 1 | 43 | `replace_all_tls()` | write | `unwrap_or_else(\|poisoned\| poisoned.into_inner())` + error 日志 | 低（已恢复） |
| 2 | 115 | `apply_partial_update()` | write | `unwrap_or_else(\|poisoned\| poisoned.into_inner())` + error 日志 | 低（已恢复） |
| 3 | 274 | `rebuild_matcher()` | read | `.expect("TLS store read lock poisoned - data integrity cannot be guaranteed")` | 高（`#[allow(dead_code)]`，当前未被调用） |
| 4 | 295 | `list_all()` | read | `.expect("TLS store read lock poisoned - ...")` | 中（诊断/API 路径） |
| 5 | 320 | `contains()` | read | `.expect("TLS store read lock poisoned - ...")` | 中（查询路径） |
| 6 | 344 | `get_validation_status()` | read | `.expect("TLS store read lock poisoned - ...")` | 中（诊断路径） |
| 7 | 369 | `get_invalid_certs()` | read | `.expect("TLS store read lock poisoned - ...")` | 中（诊断路径） |
| 8 | 397 | `get_cert_stats()` | read | `.expect("TLS store read lock poisoned - ...")` | 中（API 统计路径，`/stats` 端点） |
| 9 | 426 | `get_tls_by_host()` | read | `.expect("TLS store read lock poisoned - ...")` | 高（证书查询路径） |
| 10 | 450 | `entry_count()` | read | `.expect("TLS store read lock poisoned")` | 中（API 统计路径，`/stats` 端点） |

**小结**：2 个写路径使用恢复策略，8 个读路径使用 `expect` 直接 panic。

---

## 问题分析

### 1. 写/读路径处理策略矛盾

写路径（#1、#2）显式恢复中毒锁，设计意图是"即使之前有 panic，也尝试恢复证书数据继续服务"。但所有 8 个读路径在遇到中毒锁时直接 panic，使写端的恢复形同虚设——数据被成功恢复了，但没有任何读端能够访问它。

### 2. `std::sync::RwLock` 的 poison 标记是永久性的

关键事实：`std::sync::RwLock` 一旦被标记为 poisoned，该标记**永久存在**。即使后续写端通过 `into_inner()` 恢复了数据并成功写入，任何不处理 poison 的 `read()` 调用仍会返回 `Err(PoisonError)`。

### 3. 级联 panic 场景

```
T1 (写线程): replace_all_tls() 持有写锁 -> validate_cert 内部 panic -> 锁被标记 poisoned
T2 (写线程): apply_partial_update() -> unwrap_or_else 恢复 -> 成功写入新证书数据
T3 (API线程): get_cert_stats() -> .expect() -> PANIC
T4 (API线程): entry_count() -> .expect() -> PANIC
    ... 所有后续读操作全部 panic
```

### 4. 实际影响范围

通过代码调用链追踪，当前读路径的调用方为：

- `src/core/gateway/api/mod.rs:388-394`：`/stats` API 端点调用 `get_cert_stats()` 和 `entry_count()`。poison 后该端点将 panic。
- `get_tls_by_host()`：虽然当前无直接外部调用者（TLS 握手热路径实际走 `cert_matcher::match_sni`，后者基于 `ArcSwap` 无锁读），但作为 public API 存在，未来使用时会中招。
- `list_all()`、`contains()`、`get_validation_status()`、`get_invalid_certs()`：诊断/调试用途的 public API，poison 后全部不可用。

**注意**：TLS 握手的真正热路径 `cert_matcher::match_sni()` 使用 `ArcSwap`（无锁），不受 `RwLock` 中毒影响。因此 poison 不会直接中断 TLS 握手，但会导致：
1. `/stats` API 端点 panic（影响监控和运维可观测性）
2. 所有诊断方法不可用（影响排障能力）
3. `rebuild_matcher()` 如果未来被启用，将 panic（间接影响 matcher 重建）

### 5. 全局实例不可替换

`GLOBAL_TLS_STORE` 通过 `LazyLock` 初始化，一旦中毒，唯一恢复手段是重启进程。

---

## 最佳处理方案

### 推荐方案：迁移至 `parking_lot::RwLock`

项目已在 `Cargo.toml` 第 130 行声明 `parking_lot = "0.12"` 依赖，且在 `src/core/gateway/cache/lru.rs` 和 `src/core/gateway/routes/udp/edgion_udp.rs` 中已有使用先例。`parking_lot::RwLock` 从设计层面消除 poison 机制——`read()` / `write()` 直接返回 guard 而非 `Result`。

**修改范围**（仅 `tls_store.rs` 一个文件）：

```rust
// Before
use std::sync::{Arc, LazyLock, RwLock};

// After
use std::sync::{Arc, LazyLock};
use parking_lot::RwLock;
```

然后删除所有 poison 处理代码：

```rust
// 写路径 Before (第 43-51 行)
let mut tls_data = self.tls_data.write().unwrap_or_else(|poisoned| {
    tracing::error!( ... );
    poisoned.into_inner()
});

// 写路径 After
let mut tls_data = self.tls_data.write();
```

```rust
// 读路径 Before (第 295-298 行)
let tls_data = self.tls_data.read().expect(
    "TLS store read lock poisoned - ..."
);

// 读路径 After
let tls_data = self.tls_data.read();
```

**所有 10 个锁获取站点统一简化为 `.read()` 或 `.write()` 直接调用。**

**方案优势**：
- 从类型系统层面消除 poison 问题（API 不返回 `Result`，编译期保证）
- 代码大幅简化，删除约 20 行 poison 处理 / 日志代码
- `parking_lot` 性能通常优于 `std::sync`（无 syscall 的 fast-path spinning）
- 依赖已存在，零额外成本
- 可作为后续全项目迁移（50+ 文件、180+ 处裸 `.unwrap()`）的试点

**不推荐的替代方案**：将所有读路径统一改为 `unwrap_or_else(|poisoned| poisoned.into_inner())`。虽然消除了策略矛盾，但恢复后的数据一致性无法保证，且增加大量 boilerplate 代码。在已有 `parking_lot` 依赖的前提下没有理由选择此方案。

---

## 优先级

**P2**

理由：
- TLS 握手热路径（`cert_matcher::match_sni`）基于 `ArcSwap`，不受 `RwLock` 中毒影响，因此 poison 不会直接导致 TLS 握手失败
- 受影响的是 `/stats` API 端点和诊断方法，属于可观测性/运维层面，非数据面关键路径
- 触发前提是写锁持有期间 panic（`validate_cert` 或 `rebuild_matcher_from_data` 内部异常），概率较低
- 但写/读策略矛盾本身是一个设计缺陷，应在合理时间内修复
- 修复成本极低（单文件改动，纯机械替换），建议与下一批代码质量改进一起处理
