# TlsStore 中 RwLock Poisoning 处理策略不一致，导致级联 panic 风险

---

## 严重程度

**高 / 关键**

- **高**：写路径与读路径对 `RwLock` 中毒（poisoning）的处理策略相互矛盾——写端优雅恢复意味着开发者**期望在 poison 后继续服务**，但读端的 `expect` 会立刻触发 panic，使恢复形同虚设。
- **关键**：`TlsStore` 是 API 网关 TLS 握手的核心路径。一旦任何写锁持有线程 panic，后续**所有** HTTPS 请求的证书查询都将连锁 panic，等同于**整个 TLS 网关不可用**。

---

## 影响范围

| 维度 | 说明 |
|------|------|
| **直接受影响文件** | `src/core/gateway/tls/store/tls_store.rs` |
| **受影响结构** | `TlsStore::tls_data: RwLock<HashMap<String, TlsEntry>>` |
| **热路径影响** | `get_tls_by_host()`（每次 TLS 握手调用）、`rebuild_matcher()`、`list_all()`、`contains()`、`get_cert_stats()` 等所有读操作 |
| **上游触发点** | 任何导致 `full_set()` / `partial_update()` 持锁期间 panic 的逻辑（如 `validate_cert` 异常、`rebuild_matcher_from_data` 内部 panic） |
| **全局实例** | `GLOBAL_TLS_STORE: LazyLock<Arc<TlsStore>>`（461 行），进程唯一，不可替换 |
| **波及面** | 项目中存在 **大量** 使用 `std::sync::RwLock` 并以 `.unwrap()` 获取锁的模块（详见"项目级扫描"一节），但 `TlsStore` 是目前唯一存在**写端恢复 + 读端 panic** 这一矛盾组合的模块 |

---

## 问题详细描述

### 1. 写路径：优雅恢复 poison

`full_set()`（第 43 行）和 `partial_update()`（第 115 行）使用 `unwrap_or_else` 从中毒锁中恢复内部数据：

```43:51:Edgion/src/core/gateway/tls/store/tls_store.rs
        let mut tls_data = self.tls_data.write().unwrap_or_else(|poisoned| {
            tracing::error!(
                component = "tls_store",
                "TLS store write lock poisoned - recovering data from poisoned lock. \
                    A thread previously panicked while holding this lock."
            );
            // TODO(observability): Add metric for lock poison recovery
            poisoned.into_inner()
        });
```

```115:123:Edgion/src/core/gateway/tls/store/tls_store.rs
        let mut tls_data = self.tls_data.write().unwrap_or_else(|poisoned| {
            tracing::error!(
                component = "tls_store",
                "TLS store write lock poisoned during partial update - recovering. \
                    A thread previously panicked while holding this lock."
            );
            // TODO(observability): Add metric for lock poison recovery
            poisoned.into_inner()
        });
```

这种设计的意图是明确的：**即使之前有线程 panic，也要尝试恢复数据并继续提供服务**。对于 API 网关而言，这是合理的韧性策略——证书数据可能仍然完好，短暂的写端异常不应导致整个 TLS 层永久不可用。

### 2. 读路径：直接 panic

然而，所有 **8 个** 读操作全部使用 `.expect()` 或 `.unwrap()`，在锁中毒时直接 panic：

| 方法 | 行号 | 锁获取方式 |
|------|------|-----------|
| `rebuild_matcher()` | 274-276 | `.expect("TLS store read lock poisoned - data integrity cannot be guaranteed")` |
| `list_all()` | 295-298 | `.expect("TLS store read lock poisoned - ...")` |
| `contains()` | 319-322 | `.expect("TLS store read lock poisoned - ...")` |
| `get_validation_status()` | 343-346 | `.expect("TLS store read lock poisoned - ...")` |
| `get_invalid_certs()` | 368-371 | `.expect("TLS store read lock poisoned - ...")` |
| `get_cert_stats()` | 396-399 | `.expect("TLS store read lock poisoned - ...")` |
| `get_tls_by_host()` | 426-429 | `.expect("TLS store read lock poisoned - ...")` |
| `entry_count()` | 450 | `.expect("TLS store read lock poisoned")` |

以 `list_all()` 为例：

```295:299:Edgion/src/core/gateway/tls/store/tls_store.rs
        let tls_data = self.tls_data.read().expect(
            "TLS store read lock poisoned - a thread panicked while holding the lock. \
                     Data integrity cannot be guaranteed.",
        );
        tls_data.values().map(|entry| entry.tls.clone()).collect()
```

以 `get_tls_by_host()` 为例（TLS 握手热路径）：

```426:429:Edgion/src/core/gateway/tls/store/tls_store.rs
        let tls_data = self
            .tls_data
            .read()
            .expect("TLS store read lock poisoned - data integrity cannot be guaranteed");
```

### 3. 矛盾的后果：级联 panic

考虑以下时序：

```
T1 (写线程): full_set() 持有写锁 → validate_cert 内部 panic → 锁被标记为 poisoned
T2 (写线程): partial_update() → unwrap_or_else 恢复数据 → 成功写入新证书 ✓
T3 (读线程): get_tls_by_host("example.com") → .expect() → PANIC! ✗
T4 (读线程): list_all() → .expect() → PANIC! ✗
T5 (读线程): get_cert_stats() → .expect() → PANIC! ✗
...所有后续读操作全部 panic
```

**关键矛盾**：T2 已经成功恢复并写入了新数据，锁的内部数据实际上是一致的。但 `std::sync::RwLock` 的 poison 标记是**永久性**的——一旦设置，即使后续写入成功恢复了数据，任何不处理 poison 的 `read()`/`write()` 调用仍然会得到 `Err(PoisonError)`。

这意味着：
1. 写端的优雅恢复逻辑**完全失去意义**——虽然数据被恢复了，但没有任何读端能读到它
2. 一个写线程的 panic 会导致**所有 HTTPS 连接建立失败**
3. 由于 `GLOBAL_TLS_STORE` 是进程级唯一实例且通过 `LazyLock` 初始化，**无法重建**，唯一的恢复手段是重启进程

---

## 根因分析

### 直接原因

开发过程中写路径和读路径由不同阶段（或不同人）编写，写路径较早地考虑了韧性，读路径沿用了更常见的 `.expect()` 模式，且缺少对 poison 处理策略一致性的 review 检查。

### 深层原因

1. **`std::sync::RwLock` 的 poison 机制本身具有争议性**：Rust 标准库的 poison 设计假设 "panic = 数据可能已损坏"，但在实际工程中，许多 panic 并不会破坏锁保护的数据（例如格式化字符串失败、断言失败等）。这导致开发者对是否处理 poison 缺乏统一认知。

2. **缺少项目级锁操作规范**：项目中大量 `RwLock` 使用（50+ 个文件）全部使用裸 `.unwrap()`，没有统一的 poison 处理策略。`TlsStore` 是唯一尝试在写端做恢复的模块，但恢复做了一半。

3. **已引入 `parking_lot` 但未使用**：`Cargo.toml` 第 130 行已声明 `parking_lot = "0.12"` 依赖，但 `tls_store.rs`（及项目中所有其他模块）仍使用 `std::sync::RwLock`。`parking_lot::RwLock` 根本**不存在** poison 机制，从设计层面消除了这一问题。

---

## 项目级扫描：更广泛的 poison 风险

虽然本次 review 聚焦于 `TlsStore`，但值得注意的是整个项目的 `std::sync::RwLock` 使用情况：

| 模式 | 数量 | 典型文件 |
|------|------|---------|
| `.write().unwrap()` | 100+ 处 | `raw_store.rs`、`endpoint_store.rs`、`ep_slice_store.rs`、`gateway_route_index.rs`、`ref_manager.rs` 等 |
| `.read().unwrap()` | 80+ 处 | 同上 |
| `.write().unwrap_or_else(\|poisoned\| ...)` | **仅 2 处** | `tls_store.rs` 第 43、115 行 |
| `.read().expect("...poisoned...")` | **仅 7 处** | `tls_store.rs` 读路径 |
| 使用 `parking_lot::RwLock` | **0 处** | — |

这说明：
- `TlsStore` 是**唯一**在写端显式处理 poison 的模块
- 其余模块的裸 `.unwrap()` 在 poison 时同样会 panic，但策略至少是"一致地 panic"
- `TlsStore` 恰恰因为"只做了一半"而成为最危险的模块——它给了**虚假的韧性承诺**

---

## 修复建议

### 方案 A（推荐）：迁移至 `parking_lot::RwLock`

项目已依赖 `parking_lot = "0.12"`（`Cargo.toml` 第 130 行），直接替换即可**从根本上消除 poison 问题**。

`parking_lot::RwLock` 的设计哲学是：如果持锁线程 panic，锁会被正常释放，数据保持 panic 前的最后状态。没有 poison 标记，没有 `PoisonError`，`lock()` 永远返回 guard 而非 `Result`。

**修改后的 `tls_store.rs`：**

```rust
use parking_lot::RwLock;  // 替换 std::sync::RwLock
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

pub struct TlsStore {
    tls_data: RwLock<HashMap<String, TlsEntry>>,
}

impl TlsStore {
    pub fn new() -> Self {
        Self {
            tls_data: RwLock::new(HashMap::new()),
        }
    }

    pub fn full_set(&self, data: HashMap<String, EdgionTls>) {
        let cert_count = data.len();
        tracing::info!(
            component = "tls_store",
            count = cert_count,
            "Full set of TLS certificates"
        );

        // parking_lot::RwLock::write() 直接返回 guard，无需处理 poison
        let mut tls_data = self.tls_data.write();
        tls_data.clear();

        for (key, tls) in data {
            let validation = validate_cert(&tls);
            if !validation.is_valid {
                tracing::warn!(
                    component = "tls_store",
                    key = %key,
                    errors = ?validation.errors,
                    "Certificate validation failed"
                );
            }
            tls_data.insert(key, TlsEntry {
                tls: Arc::new(tls),
                validation,
            });
        }

        if let Err(e) = self.rebuild_matcher_from_data(&tls_data) {
            tracing::error!(
                component = "tls_store",
                error = %e,
                cert_count = cert_count,
                "CRITICAL: Failed to rebuild TLS matcher after full set"
            );
        }
    }

    pub fn list_all(&self) -> Vec<Arc<EdgionTls>> {
        // 同样简洁，无需处理 poison
        let tls_data = self.tls_data.read();
        tls_data.values().map(|entry| entry.tls.clone()).collect()
    }

    pub fn get_tls_by_host(&self, hostname: &str) -> Option<Arc<EdgionTls>> {
        let tls_data = self.tls_data.read();
        for entry in tls_data.values() {
            if !entry.validation.is_valid {
                continue;
            }
            if entry.tls.matches_hostname(hostname) {
                return Some(entry.tls.clone());
            }
        }
        None
    }

    pub fn entry_count(&self) -> usize {
        self.tls_data.read().len()
    }

    // ... 其余方法同理，所有 .read().expect(...) 替换为 .read()
    // ... 所有 .write().unwrap_or_else(...) 替换为 .write()
}
```

**方案 A 优势：**
- 代码更简洁（删除所有 poison 处理分支和错误日志）
- 从类型系统层面消除问题（API 返回 guard 而非 `Result`）
- `parking_lot` 的 `RwLock` 在性能上通常优于标准库（无 syscall 的 fast-path spinning）
- 零迁移风险：`parking_lot::RwLock` 的 API 与 `std::sync::RwLock` 几乎一致（去掉 `Result` 解包即可）
- 依赖已存在，无需新增

**迁移步骤：**
1. 将 `use std::sync::RwLock` 替换为 `use parking_lot::RwLock`
2. 删除所有 `.unwrap()`、`.expect(...)`、`.unwrap_or_else(|poisoned| ...)` 调用
3. 删除 poison 恢复相关的 `tracing::error!` 日志
4. 编译验证——`parking_lot::RwLock` 的 `read()`/`write()` 直接返回 guard，类型不匹配处编译器会报错

### 方案 B：统一使用 `unwrap_or_else` 恢复

如果团队对引入 `parking_lot::RwLock` 有顾虑（尽管已是依赖），可以将所有读路径改为与写路径一致的恢复模式：

```rust
pub fn list_all(&self) -> Vec<Arc<EdgionTls>> {
    let tls_data = self.tls_data.read().unwrap_or_else(|poisoned| {
        tracing::error!(
            component = "tls_store",
            "TLS store read lock poisoned in list_all - recovering. \
             Data may be in an inconsistent state."
        );
        poisoned.into_inner()
    });
    tls_data.values().map(|entry| entry.tls.clone()).collect()
}

pub fn get_tls_by_host(&self, hostname: &str) -> Option<Arc<EdgionTls>> {
    let tls_data = self.tls_data.read().unwrap_or_else(|poisoned| {
        tracing::error!(
            component = "tls_store",
            "TLS store read lock poisoned in get_tls_by_host - recovering. \
             Data may be in an inconsistent state."
        );
        poisoned.into_inner()
    });
    // ... 剩余逻辑不变
}

// 对所有 8 个读方法执行同样的替换
```

**方案 B 缺陷：**
- 大量重复的 boilerplate 代码（每个方法都需要 poison 恢复逻辑）
- 恢复后数据的一致性依赖于 panic 发生的具体时机，**无法保证**
- 需要额外日志/metric 来监控 poison 恢复事件
- 不解决根本问题——仍然可能读到半写入状态的数据

### 方案 C：抽取辅助方法减少重复（方案 B 的改进）

如果选择方案 B，至少应抽取公共方法：

```rust
impl TlsStore {
    fn read_tls_data(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, TlsEntry>> {
        self.tls_data.read().unwrap_or_else(|poisoned| {
            tracing::error!(
                component = "tls_store",
                "TLS store read lock poisoned - recovering data from poisoned lock"
            );
            poisoned.into_inner()
        })
    }

    fn write_tls_data(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, TlsEntry>> {
        self.tls_data.write().unwrap_or_else(|poisoned| {
            tracing::error!(
                component = "tls_store",
                "TLS store write lock poisoned - recovering data from poisoned lock"
            );
            poisoned.into_inner()
        })
    }

    // 然后所有方法统一使用：
    pub fn list_all(&self) -> Vec<Arc<EdgionTls>> {
        let tls_data = self.read_tls_data();
        tls_data.values().map(|entry| entry.tls.clone()).collect()
    }

    pub fn entry_count(&self) -> usize {
        self.read_tls_data().len()
    }

    // ...
}
```

### 推荐决策

| 维度 | 方案 A（`parking_lot`） | 方案 B/C（统一恢复） |
|------|----------------------|---------------------|
| 代码简洁度 | ★★★★★ | ★★★ |
| 数据安全性 | ★★★★★（无 poison 概念） | ★★★（恢复后数据可能不一致） |
| 性能 | ★★★★★（通常更优） | ★★★★ |
| 迁移成本 | ★★★★（依赖已存在） | ★★★★★（最小改动） |
| **推荐度** | **首选** | 过渡方案 |

**强烈推荐方案 A**。项目已声明 `parking_lot = "0.12"` 依赖但从未使用，迁移 `TlsStore` 可以作为试点，验证无误后逐步推广至其他模块。

---

## 项目级建议：逐步消除全局 poison 风险

当前项目中 50+ 个文件、180+ 处 `std::sync::RwLock` 的 `.unwrap()` 调用均存在潜在的 poison panic 风险。建议分优先级逐步迁移：

| 优先级 | 模块 | 理由 |
|--------|------|------|
| **P0** | `tls_store.rs` | 本文档所述的矛盾策略 + TLS 热路径 |
| **P1** | `endpoint_store.rs`、`ep_slice_store.rs` | 后端发现热路径，poison 会导致所有流量转发中断 |
| **P1** | `raw_store.rs` | 配置查询热路径 |
| **P2** | `cache_client/*.rs`、`cache_server/cache.rs` | 配置同步链路 |
| **P3** | 其余模块 | 逐步替换 |

迁移模式统一：
```rust
// Before
use std::sync::RwLock;
let data = self.store.read().unwrap();
let mut data = self.store.write().unwrap();

// After
use parking_lot::RwLock;
let data = self.store.read();
let mut data = self.store.write();
```

---

## 测试建议

### 1. 单元测试：验证 poison 后读路径行为（当前代码的回归测试）

```rust
#[cfg(test)]
mod poison_tests {
    use super::*;
    use std::sync::Arc;

    /// 验证当前代码在写锁 panic 后，读操作会 panic（证明问题存在）
    #[test]
    #[should_panic(expected = "TLS store read lock poisoned")]
    fn read_panics_after_write_poison() {
        let store = Arc::new(TlsStore::new());
        let store_clone = store.clone();

        // 在写锁持有期间故意 panic，使锁中毒
        let handle = std::thread::spawn(move || {
            let _guard = store_clone.tls_data.write().unwrap();
            panic!("intentional panic while holding write lock");
        });
        let _ = handle.join(); // 等待线程结束（忽略 panic）

        // 此处应当 panic — 证明 bug 存在
        let _ = store.list_all();
    }

    /// 验证写端能恢复但读端不能（证明不一致）
    #[test]
    fn write_recovers_but_read_panics() {
        let store = Arc::new(TlsStore::new());
        let store_clone = store.clone();

        // 中毒
        let handle = std::thread::spawn(move || {
            let _guard = store_clone.tls_data.write().unwrap();
            panic!("intentional");
        });
        let _ = handle.join();

        // 写端恢复成功
        store.full_set(HashMap::new()); // 不会 panic

        // 读端仍然 panic（即使写端刚恢复了数据）
        let result = std::panic::catch_unwind(|| store.entry_count());
        assert!(result.is_err(), "读端应当 panic 但没有");
    }
}
```

### 2. 修复后的验证测试

```rust
#[cfg(test)]
mod fixed_tests {
    use super::*;

    /// 修复后：写锁 panic 不影响后续读写
    #[test]
    fn reads_work_after_write_poison() {
        let store = Arc::new(TlsStore::new());
        let store_clone = store.clone();

        let handle = std::thread::spawn(move || {
            let _guard = store_clone.tls_data.write();
            panic!("intentional");
        });
        let _ = handle.join();

        // parking_lot: 读操作正常工作
        assert_eq!(store.entry_count(), 0);
        assert!(store.list_all().is_empty());
        assert!(!store.contains("any-key"));
        assert_eq!(store.get_cert_stats(), (0, 0));
    }

    /// 修复后：写锁 panic 后仍可写入新数据
    #[test]
    fn writes_work_after_write_poison() {
        let store = Arc::new(TlsStore::new());
        let store_clone = store.clone();

        let handle = std::thread::spawn(move || {
            let _guard = store_clone.tls_data.write();
            panic!("intentional");
        });
        let _ = handle.join();

        // 写入新数据
        let mut data = HashMap::new();
        data.insert("test/cert".to_string(), make_test_tls());
        store.full_set(data);

        // 验证数据可读
        assert_eq!(store.entry_count(), 1);
        assert!(store.contains("test/cert"));
    }
}
```

### 3. 压力测试：并发读写 + panic 注入

```rust
#[test]
fn concurrent_reads_survive_writer_panic() {
    let store = Arc::new(TlsStore::new());

    // 预填充一些证书
    store.full_set(make_test_certs(10));

    let mut handles = vec![];

    // 启动多个持续读取的线程
    for _ in 0..4 {
        let s = store.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..1000 {
                let _ = s.list_all();
                let _ = s.entry_count();
                let _ = s.get_cert_stats();
                std::thread::yield_now();
            }
        }));
    }

    // 启动一个会 panic 的写线程
    let s = store.clone();
    let panic_handle = std::thread::spawn(move || {
        let _guard = s.tls_data.write();
        panic!("writer panic");
    });
    let _ = panic_handle.join();

    // 所有读线程应正常完成（不 panic）
    for h in handles {
        h.join().expect("读线程不应 panic");
    }
}
```

### 4. 集成测试建议

- 在 staging 环境模拟证书更新期间的异常（如传入畸形证书数据触发 `validate_cert` 内部异常）
- 验证异常后 TLS 握手仍可正常完成
- 监控 `tracing::error` 日志中 "lock poisoned" 关键字，确认生产环境是否已出现过 poison 事件

---

## 小结

| 项 | 结论 |
|----|------|
| **根因** | 写路径使用 `unwrap_or_else` 恢复 poison，读路径使用 `.expect()` 直接 panic，策略矛盾 |
| **风险** | 任一写线程 panic → 锁永久中毒 → 所有读操作级联 panic → **TLS 网关完全不可用** |
| **核心矛盾** | 写端的恢复逻辑形同虚设——数据恢复了但没有读端能访问 |
| **推荐修复** | 迁移至 `parking_lot::RwLock`（依赖已存在），从类型系统层面消除 poison 问题 |
| **项目级影响** | 50+ 文件的 `std::sync::RwLock` 裸 `.unwrap()` 调用均有类似风险，建议以 `TlsStore` 为试点逐步迁移 |

---

*文档基于仓库当前实现撰写；行号以撰写时 `Edgion` 主分支为准，若后续改动请以实际文件为准。*
