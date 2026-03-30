# Workqueue `enqueue` / `enqueue_after` TOCTOU 竞态条件 — 架构评审

**评审对象**：`src/core/controller/conf_mgr/sync_runtime/workqueue.rs`  
**主题**：`pending` / `scheduled` 与 `DashSet` 上的「先检查后行动」非原子性，及其与 channel、`metrics.depth`、delay 子系统的交互。  
**结论摘要**：去重在高并发下**可被绕过**，会导致**同一 key 在 ready channel 或 delay heap 中重复出现**；`depth`（及 delay 路径上的计数）**可能系统性偏低或出现下溢观感**。属于**真实逻辑缺陷**，而非纯理论问题；修复成本较低（以 `insert` 返回值或单 shard 互斥统一「准入」）。

---

## 1. 代码与不变量回顾

### 1.1 设计意图

模块注释与 `enqueue` / `dequeue` 语义表明：

- **`pending`**：表示 key 当前「已在 ready 队列侧登记」（与 channel 中未消费项对应，设计上应一一对应）。
- **不去重 processing**：出队时立刻 `pending.remove`，以便处理过程中再次 `enqueue`（dirty requeue），这是**刻意的**窗口，与本文讨论的 TOCTOU **不是同一类问题**。
- **`scheduled`**：延迟路径上的去重，与 `pending` 联合用于 `enqueue_after`。

### 1.2 问题代码形态（概念）

当前 `enqueue` 与 `enqueue_after` 的核心模式为：

1. `contains`（读）
2. `insert`（写）
3. 再向 `mpsc` / delay channel 发送

在 **1 与 2 之间**、以及 **delay 循环内类似的 `contains` → `insert` 之间**，其他任务可插入，构成经典 **TOCTOU**。

---

## 2. TOCTOU 的实际影响（生产是否「真的会坏」）

### 2.1 对「单 key 单通道项」不变量的破坏

**会。** `DashSet` 保证并发安全，但**不**保证「`contains` 与 `insert` 组合」的原子性。两任务可交错为：

| 步骤 | 任务 A | 任务 B |
|------|--------|--------|
| 1 | `contains(k)` → false | |
| 2 | | `contains(k)` → false |
| 3 | `insert(k)` | |
| 4 | | `insert(k)` → 集合中仍只有一个 `k` |
| 5 | `tx.send(...)` | `tx.send(...)` |

结果：**channel 内有两条同 key 的 `WorkItem`，`pending` 仅一条。** 去重语义在「队列内」层面失效。

### 2.2 对协调器行为的影响

影响取决于上层 reconcile **是否幂等**：

- 若对同一 key 连续处理两次**仅多费 CPU、结果一致**：生产上多为**可观测性 + 负载**问题（重复 reconcile、重复 API 调用）。
- 若存在「非幂等副作用」或依赖「同一时刻最多一个在飞」的隐含假设：可能出现**重复写、乱序假设被打破**等更难排查的问题。

因此：**在生产中是否「出事」取决于业务幂等性；去重契约在代码层面已被打破，属于应当修复的缺陷。**

### 2.3 与「dirty requeue」注释的边界

`dequeue` 后 `pending` 已清空该 key，此时 `enqueue` 成功是设计行为。  
TOCTOU 导致的是：**在 `pending` 仍应表示「已在队列中」的时段内**，重复放入 channel。二者时间窗口与语义不同，**不能**用 dirty requeue 合理化 TOCTOU。

---

## 3. 竞态场景与触发概率

### 3.1 `enqueue` 与 `enqueue` 之间

- **条件**：同一 `key` 在极短时间被多个异步任务 `enqueue`（例如同一资源的多路 watch、burst 事件、多 handler 同时触发）。
- **概率量级**：与「每 key 的并发 `enqueue` 调用率」及「`contains`→`insert` 窗口长度」成正比。负载高、热点 key 多时，**低概率事件会在规模上变为必现**（毛刺期间重复入队）。

### 3.2 `enqueue` 与 delay 循环（`spawn_delay_loop`）

延迟任务到期路径中（约 `scheduled.remove` 之后）存在 **`pending.contains` → `pending.insert` → `inc_depth` → `ready_tx.send`** 序列。  
若与并发 `enqueue` 交错，可能出现：

- 集合中已有 `k`，但延迟路径仍 **`inc_depth` 并再次 `send`**（若实现未根据 `insert` 返回值分支），或
- 与双 `enqueue` 类似的双重 `send`。

即：**delay → ready 与立即 `enqueue` 共享同一套 TOCTOU 模式时，会放大重复项与计数偏差。**

### 3.3 `enqueue_after` 与 `enqueue_after` / `enqueue`

`enqueue_after` 对 `pending` 与 `scheduled` 做**联合读**，再 `scheduled.insert`，再 `delay_tx.send`：

- 两任务可同时通过 `contains` 检查，两次 `scheduled.insert(k)`：**仅一个**在集合中新增元素（`DashSet::insert` 对重复 key 返回 `false`），但当前实现**若未检查返回值**，仍可能 **`inc_delayed` 两次、向 delay channel 发送两个 `DelayedItem`**，堆上出现**重复 key** 的延迟项（直至到期路径逐步消化）。

---

## 4. `dequeue` 与 `metrics.depth` 偏差

### 4.1 重复 channel 项与 `pending.remove`

第一次 `dequeue`：`recv` 得第一条 → `pending.remove(k)` 成功 → `dec_depth` 一次。  
第二次 `dequeue`：同 key 第二条 → `pending` 中**已无** `k`，但实现仍会 **`dec_depth`**。

`WorkqueueMetrics::dec_depth` 使用 `fetch_sub(1, Relaxed)`，**无下界**。因此在存在重复 channel 项时：

- **`depth` 可能长期低于真实 channel 长度**（若某些路径多 `inc` 少 `dec` 或反之，组合后形态更乱），并可能出现 **UInt 下溢成极大正数** 的观测（取决于读取方式与是否做饱和）。

### 4.2 与 `adds_total`、`delayed_total` 的关系

- `enqueue` 成功路径：`inc_adds` 与 `inc_depth` 与「是否重复项」可能不一致。
- `enqueue_after` 在 `delay_tx.send` 失败时移除 `scheduled`，但 **`delayed_total` 的递增若未对称回滚**，会与 `scheduled` 集合不一致（独立小问题，可与 TOCTOU 修复一并梳理）。

### 4.3 可接受度（评审意见）

- **若 `depth` 仅用于日志/告警趋势**：严重下溢时**告警失真**，可能误导运维。
- **若用于背压或容量判断**：当前实现已与真实队列长度**脱钩**，TOCTOU 会加剧偏差。
- **建议**：将 **`depth` 视为不可靠**直至准入逻辑原子化；修复后应用 **`insert` 成功与否** 决定是否 `inc_depth` / `send`，并在 `dequeue` 路径考虑 **`remove` 返回值** 决定是否 `dec_depth`（或改用「仅由 send 方与 recv 方对称记账」的单一数据源）。

---

## 5. 修复方案（按推荐顺序）

### 5.1 方案 A：以 `DashSet::insert` 的返回值作为唯一准入闸门（推荐）

语义：**只有第一个把 key 放进集合的调用者才有资格 `send`。**

伪代码思路：

```rust
// enqueue
if !self.pending.insert(key.clone()) {
    return false; // 已存在，真正的去重
}
self.metrics.inc_depth();
match self.tx.send(item).await {
    Ok(()) => { self.metrics.inc_adds(); true }
    Err(e) => {
        self.pending.remove(&key);
        self.metrics.dec_depth();
        false
    }
}
```

要点：

- **删除**单独的 `contains` + `insert` 两步，避免 TOCTOU。
- `insert` 返回 `false` 时**不**增减 `depth`，直接返回「未入队」。

`dequeue` 侧可选优化：

- `if self.pending.remove(&item.key) { self.metrics.dec_depth(); }`  
  避免「重复 channel 项」时二次 `dec_depth`（修复 TOCTOU 后理论上不应出现重复项，但防御性有益）。

### 5.2 方案 B：单 shard / 每 key 互斥（过重，一般不必上）

对 `key` 做 hash 分桶 + `tokio::sync::Mutex` 或 `mpsc` 串行化 per-key 操作，可完全消除并行 `enqueue` 交错，但复杂度高，通常 **不如方案 A**。

### 5.3 方案 C：`enqueue_after` 的联合集合语义

需要在**一次原子决策**中表达「既不在 `pending` 也不在 `scheduled`」：

- **实用做法**：先 `scheduled.insert`；若 `insert` 为 `false`，直接返回 false。若 `insert` 为 `true`，再检查 `pending`：若已在 `pending`，则 **`scheduled.remove` 回滚** 并返回 false（顺序可论证避免死锁：与 delay 循环的 `scheduled.remove` / `pending.insert` 需对照设计，避免互相卡住）。
- 更干净的做法是引入**单一 `DashSet` 或三态 map**（`Idle | Pending | Scheduled`）用一次 `entry` 式 API 更新，但改动面更大。

**delay 循环**中与 `pending` 相关的 `contains` + `insert` 应同样改为 **`insert` 返回值驱动**，仅在「新插入」时 `inc_depth` 并 `send`，与 `enqueue` 一致。

### 5.4 测试建议

- 并发压测：多任务对**同一 key** 同时 `enqueue`，断言 channel 中**至多一条**未消费项（或 `len()` / 自定义探测与 `pending` 一致）。
- `enqueue_after` 并发：同 key 多次 `enqueue_after`，断言 delay 侧**无重复 heap 项**或到期后 ready 侧仍满足去重。
- 修复后跑现有 `test_deduplication` 与扩展的并发用例。

---

## 6. 对 delay queue 的影响小结

| 环节 | 风险 |
|------|------|
| `enqueue_after` | 联合 `contains` 非原子 → 重复 `DelayedItem`、重复 `inc_delayed`、堆内重复 key |
| `spawn_delay_loop` 到期 | `pending.contains` → `insert` 非原子 → 与 `enqueue` 交错时重复 `send` / 错误 `inc_depth` |
| `scheduled.remove` 时机 | 与 `insert` 回滚策略一起设计，避免「scheduled 已删但 ready 未入」的短暂窗口被误判（方案 C 需统一说明） |

**结论**：delay 子系统**不是**独立于 TOCTOU 的；相反，因多任务 + 后台循环，**交错更多**，更应在同一轮修复中**统一「准入 = insert 成功」**模型。

---

## 7. 总结表

| 维度 | 评估 |
|------|------|
| 是否理论问题 | 否；可破坏去重不变量并搞乱指标 |
| 生产是否必现 | 低单点概率、高规模/热点下显著 |
| 业务后果 | 依赖 reconcile 幂等；通常重复工作 + 指标不可信 |
| 推荐修复 | 用 `DashSet::insert`（及必要时 `remove`）返回值统一准入；delay 路径同样处理 |
| `depth` 可接受度 | 修复前仅作粗参考；修复后应对齐 channel 语义并考虑 `dequeue` 条件 `dec_depth` |

---

## 8. 参考代码位置

- `Workqueue::enqueue`：约 L418–466  
- `Workqueue::enqueue_after`：约 L475–525  
- `Workqueue::dequeue`：约 L534–549  
- `spawn_delay_loop`：`pending`/`scheduled` 与 `ready_tx.send`：约 L334–363 及关闭分支 L374–384  

以上行号以评审时文件版本为准，后续重构请以符号搜索为准。
