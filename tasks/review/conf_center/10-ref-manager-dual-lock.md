# BidirectionalRefManager 双 RwLock 分离：一致性风险与死锁评审

**文档类型**：架构 / 并发安全评审  
**关联代码**：[`ref_manager.rs`](../../../src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_manager.rs)  
**严重程度**：**P0**（真实死锁可能 + 索引短暂不一致）  
**日期**：2026-03-30

---

## 1. 背景与数据模型

`BidirectionalRefManager<V>` 维护双向多对多索引：

| 字段 | 语义 |
|------|------|
| `refs`（正向） | `source_key → HashSet<V>`：哪些 **value** 依赖该 **source** |
| `dependencies`（反向） | `value_key → HashSet<source_key>`：某个 **value** 依赖哪些 **source** |

具体实现为两把独立的 `std::sync::RwLock<HashMap<...>>`。模块注释声明「所有变更操作保持两索引同步」，但实现上**任意跨两把锁的更新都不是原子操作**，且**写路径之间存在锁顺序不一致**。

以下类型均为 `BidirectionalRefManager<ResourceRef>` 的别名，**共享同一套并发语义与缺陷**：

- `SecretRefManager`（K8s Controller 中为**全局共享** `Arc`，多 `ResourceController` 共用）
- `ServiceRefManager`（全局 `LazyLock`）
- `CrossNamespaceRefManager`（`OnceLock<Arc<...>>`）

---

## 2. 各方法加锁顺序一览

为便于对照，将当前实现中的锁获取顺序整理如下（仅列出同时触及两把锁的路径）。

| 方法 | 顺序 | 锁类型 |
|------|------|--------|
| `add_ref` | `refs` → `dependencies` | 写 → 写 |
| `remove_ref` | `refs` → `dependencies` | 写 → 写 |
| `clear_value_refs` | `dependencies` → `refs` | 写 → 写 |
| `clear` | `refs` → `dependencies` | 写 → 写 |
| `stats` | `refs` → `dependencies` | 读 → 读 |

**结论**：`clear_value_refs` 与**其余所有双锁写路径及 `stats` 双读路径**在「先锁哪一张表」上存在**顺序反转**，这是死锁与不原子快照的根源。

---

## 3. AB-BA 写锁死锁：精确条件

### 3.1 经典双写场景（`add_ref` / `remove_ref` / `clear` vs `clear_value_refs`）

设：

- 线程 **A** 执行 `add_ref`（或 `remove_ref`，或 `clear`）：已持有 **`refs` 的写锁**，正在等待 **`dependencies` 的写锁**。
- 线程 **B** 执行 `clear_value_refs`：已持有 **`dependencies` 的写锁**，正在等待 **`refs` 的写锁**。

则 **A 等 B、B 等 A**，构成 **AB-BA 循环等待**，**必然死锁**（与是否 async 无关）。

**触发条件（充分条件）**：

1. 同一 `BidirectionalRefManager` 实例上；
2. 并发执行「先 `refs` 后 `dependencies`」的写操作与 `clear_value_refs`；
3. 两条路径分别已拿到第一把写锁、尚未拿到第二把。

**是否会在当前调用模式下发生？**

- **会。** K8s `controller.rs` 中 `SecretRefManager` 为**全局共享**；多个资源类型的 reconcile / worker 线程可在**同一进程内**同时对同一 manager 调用 `add_ref`（parse）与 `clear_value_refs`（更新前清理、删除）。`ServiceRefManager`、`CrossNamespaceRefManager` 同理存在「多 handler、多资源并行处理」的架构空间。
- 死锁为**竞态**：负载越高、并行度越大，**偶发卡死**概率上升，**难用单测稳定复现**，但模型上**无额外条件**即可成立，不应依赖「实际从未撞到」作为安全论证。

### 3.2 `clear` 与 `clear_value_refs` 并发

`clear` 顺序为 `refs` → `dependencies`，与 `clear_value_refs`（`dependencies` → `refs`）同样构成 **AB-BA**。例如 reload / `clear_registry` 清空全局索引的同时，某 worker 正在对同一 manager 做 `clear_value_refs`，即可死锁。

---

## 4. 读锁与写锁：`stats` 与 `clear_value_refs` 的额外死锁

既有文档中曾认为「双读与写不构成经典死锁」，在 **`std::sync::RwLock` 且 `stats` 固定先 `refs` 后 `dependencies` 读**的前提下，需单独核对与 **`clear_value_refs`（先写 `dependencies` 再写 `refs`）** 的交错。

**构造：**

1. 线程 **S** 执行 `stats()`：已获得 **`refs.read()`**，尝试 **`dependencies.read()`**。
2. 线程 **C** 执行 `clear_value_refs`：已获得 **`dependencies.write()`**，尝试 **`refs.write()`**。

此时：

- **C** 的 `refs.write()` 必须等待 **S** 释放 `refs.read()`；
- **S** 的 `dependencies.read()` 必须等待 **C** 释放 `dependencies.write()`。

形成循环等待，**死锁**。

因此风险不仅是「写 vs 写」，还包括 **「先读后读」的观测路径 `stats`」与「先写后写的清理路径」** 在同一实例上的并发。任何监控、调试或健康检查若在运行时调用 `stats()`，与业务路径上的 `clear_value_refs` 重叠，即存在卡死可能。

---

## 5. 中间状态不一致（撕裂快照）

### 5.1 写路径之间的窗口

以 `add_ref` 为例（`remove_ref` 同理）：

1. 在 **`refs` 写锁**内插入 `source_key → value`；
2. 释放 `refs` 写锁；
3. 再获取 **`dependencies` 写锁**，插入 `value_key → source_key`。

在步骤 2–3 之间，另一线程可执行只读操作：

- `get_refs(source_key)` 已能看到新 `value`；
- `get_dependencies(value_key)` 仍**看不到**对应 `source_key`。

即 **正向索引已更新、反向索引尚未更新**。

### 5.2 对业务语义的影响

依赖关系用于 **Secret/Service/跨命名空间变更时的级联 requeue**。短暂不一致可能导致：

- **漏 requeue**：若某路径仅扫反向索引，可能在窗口期内**看不到**新依赖（直到下一次事件或全量触发）。
- **重复/乱序**：与 workqueue 去重、重试策略叠加时，多为**短暂**现象，但在高 churn 下放大为「时有时无」的疑难行为。

此类问题**非崩溃型**，但与死锁同属**正确性缺陷**，且更难从日志直接归因。

### 5.3 `stats()` 的非原子快照

`stats` 先后持有 `refs.read()` 与 `dependencies.read()`，两次读之间另一线程可修改任一表，故 **source_count / value_count / total_references 三者之间无事务保证**。通常仅影响观测与告警；若未来用 `stats` 做控制面决策，需知晓该语义。

---

## 6. 修复方案对比

### 6.1 统一双写锁顺序（最小侵入思路）

例如规定：**任意同时需要两把写锁时，始终先 `dependencies` 后 `refs`**（或统一为先 `refs` 后 `dependencies`，任选其一，**全文件一致**）。

- **必须**调整的方法至少包括：`add_ref`、`remove_ref`、`clear_value_refs`、`clear`。
- **`stats`**：为与写锁顺序兼容、避免 4 节所述读写死锁，**双读顺序应与写锁全局顺序一致**（例如若写锁全局为 `deps` → `refs`，则 `stats` 也应先 `dependencies.read()` 再 `refs.read()`）。

**优点**：改动面相对集中；仍保留读写分离的「读可并行」潜力（两把锁上仍可并发读不同键——但跨表一致性仍仅靠顺序约定，**单操作内**两表更新之间仍有窗口，除非在持第一把写锁期间**不释放**即去取第二把，见下条）。

**注意**：若仍采用「先释放第一把锁再取第二把」，**撕裂窗口**仍然存在；要消除窗口需在**不释放第一把写锁的情况下**嵌套获取第二把写锁，这会**长期占用两把写锁**，增大阻塞面；更干净的做法是单锁（6.2）。

### 6.2 合并为单把 `RwLock` 保护结构化状态（推荐基线）

```text
RwLock<RefMaps<V>>  其中 RefMaps { refs: HashMap<...>, dependencies: HashMap<...> }
```

**优点**：

- **单一临界区**：每次 `add_ref` / `remove_ref` / `clear_value_refs` / `clear` / `stats` 在**一次**加锁内完成，**无 AB-BA**、**无 `stats` 读写反转**、**无跨锁撕裂**（在「一次操作」语义下两表一致）。
- API 可保持同步、Poison 行为与现有一致（仍可用 `parking_lot` 替换以换性能，见 6.4）。

**缺点**：

- 所有读路径共享一把锁；**读并发**为全局互斥（`RwLock` 读仍可多读者，但同一结构体上会与写竞争）。在 map 尺寸与 QPS 适中时通常可接受；若 profiling 显示热点，再考虑 6.3。

### 6.3 分片 / `DashMap` 等细粒度并发

- **两把 `DashMap`**：若仍分两张表分别更新，**若不使用额外全局顺序或事务**，仍可能出现**反向索引与正向索引不一致**；需自行设计「按 key 顺序加锁」或「单事务层」，复杂度往往不低于单锁。
- **按 `source_key` / `value_key` 分片**：可降低争用，但实现与测试成本高，适合确认单锁瓶颈后再做。

### 6.4 `parking_lot::RwLock`

与「单锁 / 双锁」正交：替换 `std::sync::RwLock` 可改善延迟与 writer 公平性，并避免 poison，但**不解决锁顺序错误**；应在**锁模型正确**的前提下作为性能或可观测性优化。

---

## 7. 性能权衡小结

| 方案 | 死锁风险 | 索引一致（单次操作内） | 读扩展性 | 实现成本 |
|------|----------|------------------------|----------|----------|
| 现状双锁 | **有**（写-写 + 读写） | 否（跨锁窗口） | 两表可并行读 | — |
| 统一顺序 + 连续持双写 | 可消除 AB-BA（若 `stats` 同步调整） | 若嵌套持锁可减轻窗口 | 仍有限 | 中 |
| **单 `RwLock` 结构体** | **无顺序问题** | **是** | 单锁争用 | **低～中** |
| 分片 / 无锁结构 | 依赖设计 | 需严格证明 | 高 | 高 |

对当前 Conf Center 体量，**优先推荐单 `RwLock<RefMaps<V>>`**；若后续 profiling 证明 ref 管理成为瓶颈，再在不变更对外语义的前提下做分片或读多写少优化。

---

## 8. 推荐方案与实施步骤

### 8.1 推荐

1. **短期（P0）**：将 `refs` 与 `dependencies` **合并为单把 `RwLock`** 保护的私有结构体，所有公开方法在**一次**加锁内读写两表（或明确极短的组合原语）。
2. **配套**：增加**并发压力测试**（多线程交错 `add_ref`、`remove_ref`、`clear_value_refs`、`clear`、`stats`），在修复前可预期偶发挂起，修复后应**无死锁**。
3. **文档**：在 `ref_manager.rs` 模块级注释中写明「历史问题：双锁顺序反转导致死锁；现由单锁保证原子更新与固定顺序」。

### 8.2 实施步骤（建议顺序）

1. 定义 `struct RefMaps<V> { refs: HashMap<...>, dependencies: HashMap<...> }`（或内嵌于 manager 的私有字段）。
2. 将 `BidirectionalRefManager` 改为 `inner: RwLock<RefMaps<V>>`（字段名自定）。
3. 逐个迁移 `add_ref` / `remove_ref` / `clear_value_refs` / `clear` / `get_refs` / `get_dependencies` / `all_source_keys` / `stats`，保证逻辑与现有一致，**临界区内完成配对更新**。
4. 运行现有单元测试（`ref_manager.rs` 内已有覆盖）并新增 `#[test]` 或多线程 `stress` 测试（可使用 `std::thread::scope` 或 `rayon` 若已在依赖中）。
5. （可选）在 CI 中标记该 stress 测试为 `ignored` 或仅在 `--features stress` 下运行，避免拖慢默认 `cargo test`。
6. 全量 `cargo test` / 相关集成测试；观察级联 requeue 相关用例。

### 8.3 不建议单独采用的方案

- **仅**将 `clear_value_refs` 改为与 `add_ref` 相同顺序、但**仍分两段释放锁**：仍可保留**中间撕裂**，且易在后续新增方法时再次引入顺序错误。
- **仅**换 `parking_lot` 而不改锁粒度或顺序：**死锁与不一致仍存在**。

---

## 9. 结论

1. **死锁**：`clear_value_refs` 与 `add_ref` / `remove_ref` / `clear` 之间存在 **AB-BA 写锁死锁**；`clear_value_refs` 与 **`stats` 双读**之间存在 **读写循环等待死锁**。在当前「全局共享 RefManager + 多资源并行处理」模式下，**具备真实触发条件**，不应视为理论风险。
2. **一致性**：双锁分步更新导致 **refs 与 dependencies 短暂不一致**；`stats` 为 **非原子快照**。
3. **修复**：**合并为单 `RwLock` 保护双表** 是性价比最高的正确性修复；统一双锁顺序可作为理解过程中的中间分析，但**工程上更推荐单锁**以避免未来回归。
4. **影响面**：所有基于 `BidirectionalRefManager` 的全局/共享实例均受影响，修复一处实现即可惠及 `SecretRefManager`、`ServiceRefManager`、`CrossNamespaceRefManager`。

---

## 10. 参考

- 实现源码：`src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_manager.rs`
- 相关前置讨论：`tasks/review/conf_center/04-sync-locks-in-async-context.md`（其中关于「`stats` 双读不与写构成死锁」的表述在 **`clear_value_refs` 先写 `dependencies` 后写 `refs`** 的交错下**不成立**，应以本文第 4 节为准。）
- 全局单例与注入关系：`tasks/review/conf_center/01-global-singletons.md`
