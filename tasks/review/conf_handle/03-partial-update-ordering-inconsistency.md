# 问题标题

各路由协议 `ConfHandler::partial_update` 中 **删除 / 更新 / 新增** 的应用顺序不一致，且 TLS 与 TCP/UDP 采用相反的整体策略，增加维护成本并可能引入边界行为差异。

**严重程度：** 中

---

## 影响范围

| 维度 | 说明 |
|------|------|
| **代码位置** | `GlobalHttpRouteManagers`、`GlobalGrpcRouteManagers`、`GlobalTcpRouteManagers`、`GlobalTlsRouteManagers`、`GlobalUdpRouteManagers` 的 `partial_update` 实现 |
| **文件** | `src/core/gateway/routes/http/conf_handler_impl.rs`、`grpc/conf_handler_impl.rs`、`tcp/conf_handler_impl.rs`、`tls/conf_handler_impl.rs`、`udp/conf_handler_impl.rs` |
| **行为面** | 配置增量同步时路由缓存与按 Listener 重建的语义；若上游在同一批次中组合非常规操作（或未来扩展中间态观测），不同顺序可能导致短暂不一致或难以推理的状态 |
| **维护面** | 新协议或复制粘贴实现时易选错顺序；排查「为何仅 TLS 行为不同」成本高 |

---

## 各协议处理顺序详细对比表

下表按 **单次 `partial_update(add, update, remove)` 调用内** 的实际步骤归纳（不含各协议特有的 LB、units 解析等附加逻辑）。

| 协议 | 受影响 Listener 收集时机 | 对 `remove` 集合 | 对 `update` 集合 | 对 `add` 集合 | 路由主缓存写入顺序（概括） | 与 Listener 表重建关系 |
|------|-------------------------|------------------|------------------|---------------|---------------------------|-------------------------|
| **HTTP** | 在写入缓存**之前**：对合并后的 add+update 的新旧 listener 求并集，并对 remove 键的旧路由求 listener | 在 add/update **之后** 从 `route_cache` 删除 | 与 add **合并**为 `add_or_update`，再统一 `insert`（`update` 覆盖同名 `add`） | 同上合并后统一 `insert` | 先批量 `insert` 合并映射，再按 `remove` 删除 | 最后 `rebuild_affected_listener_managers` |
| **gRPC** | 同 HTTP：写入**前**收集 | 与 units 缓存一并 `remove`（在 add/update 写入**之后**） | 经 `chain(add, update)` 合并后写入 `route_cache` + `route_units_cache` | 同上 | 先写 add/update 批次，再删 remove | 最后 `rebuild_affected_listener_managers` |
| **TCP** | 在改缓存**之前**：对 `remove` 与 `update` 键的**旧**路由收集 listener；`update`/`add` 成功后追加新 listener | **最先** `remove_route` | **其次** `remove_route` + `initialize_route` + `insert_route` | **最后** `initialize_route` + `insert_route` | **显式** remove → update → add | 最后 `rebuild_affected_listener_managers` |
| **TLS** | 同 TCP/UDP：先收集 remove/update 的旧 listener | **最后** `remove_route` | **中间** `remove_route` + `initialize_route` + `insert_route` | **最先** `initialize_route` + `insert_route` | **add → update → remove**（与 TCP/UDP 相反） | 最后 `rebuild_affected_listener_managers` |
| **UDP** | 同 TCP | **最先** `remove_route` | **其次** 同 TCP update 路径 | **最后** 同 TCP add 路径 | **显式** remove → update → add | 最后 `rebuild_affected_listener_managers` |

### 代码锚点（便于对照）

- **HTTP**：`add_or_update = add` 后 `extend(update)`；先算 `affected_listeners`，再 `insert` 合并映射，最后 `remove`（见 `conf_handler_impl.rs` 约 161–217 行）。
- **gRPC**：`chain(add, update)` 合并并 `preparse`；先 `affected_listeners`，再写入双缓存，最后删 remove（约 84–131 行）。
- **TCP**：`remove` → `update`（先 `remove_route` 再 `insert_route`）→ `add`（约 68–126 行）。
- **TLS**：`add` → `update` → `remove`（约 66–121 行）。
- **UDP**：与 TCP 同序（约 68–126 行）。

---

## 问题分析

### 1. 存在三类「策略」，而非一种

1. **HTTP / gRPC：合并 add+update，批量写缓存，remove 在写缓存阶段靠后**  
   - 优点：实现短、易读；`affected_listeners` 在写缓存前已结合旧值计算，最终重建集合较完整。  
   - 缺点：**add 与 update 在实现层被合并**，若未来需要区分「仅新增」与「仅变更」的钩子（指标、审计、差异化预热），需额外拆分；与 L4 三协议的「分阶段」心智模型不一致。

2. **TCP / UDP：先删、再改、再加（remove → update → add）**  
   - 优点：与常见「先腾出再写入」一致；对**同一 listener 上资源互斥或唯一性**类不变式，通常更保守（先去掉待删/待替换的旧绑定，再写入新状态）。  
   - 缺点：与 HTTP/gRPC 的「一张合并表」写法不同，新人需切换两套叙事。

3. **TLS：先加、再改、最后删（add → update → remove）**  
   - 与 TCP/UDP **顺序相反**。在**最终仍会统一 `rebuild_affected_listener_managers`** 的前提下，多数批次在**终态**上与「先删后加」可能等价；但若 `insert_route` / `remove_route` 或中间读路径对**临时重叠**敏感（例如同一 listener 上旧键未删时新键已入缓存），TLS 的中间态与 TCP/UDP 不同，**更难以形式化证明与 TCP 行为一致**。  
   - 从**工程一致性**角度，应视为缺陷倾向：同一产品内 L4 路由不应在无注释、无测试证明的情况下采用相反顺序。

### 2. 维护与复制粘贴风险

- 修复或新增协议时，开发者容易参照「最近改过的文件」；TLS 与 TCP/UDP 顺序相反，**极易在合并或 cherry-pick 时混入错误顺序**。  
- Code Review 难以用「扫一眼」确认：必须逐行比对三个阶段顺序。

### 3. HTTP 与 gRPC 的合并细节差异

- HTTP 使用 `add_or_update.extend(update)`：**同一 key 同时出现在 `add` 与 `update` 时，以 `update` 为准**（后者覆盖）。  
- gRPC 使用 `chain(add, update)` 再 `insert`：**同一 key 时同样后写覆盖**。  
二者语义接近，但与「显式分 add/update」的 API 表面仍不完全对齐，文档化不足时易产生误解。

### 4. 为何严重程度评为「中」而非「高」

- 当前实现均在批次末尾触发按 Listener 重建，**多数场景终态一致**；未发现仅凭静态阅读即可断定的必现生产逻辑错误。  
- 不一致性主要抬高 **认知负担、演进风险与证明成本**；TLS 顺序与兄弟实现相反，属于 **应优先对齐的实质性技术债**。

---

## 统一方案建议

**建议以 TCP/UDP 的语义为基准，统一为：**

1. **（可选但推荐）在修改任何路由缓存之前**，收集 `remove` 与 `update` 涉及键的**旧** `resolved_listeners`（或协议等价物），并合并 `update`/`add` 成功后的新 listeners，得到完整的 `affected_listeners`。  
2. **应用顺序：`remove` → `update` → `add`**（对 L4 已与 TCP/UDP 一致；TLS 应改为该顺序）。  
3. **HTTP/gRPC**：在保持「先算 affected、再一次性重建」的前提下，可将缓存写入阶段也调整为与上相同的**逻辑顺序**（例如先处理 remove 键，再处理 update，再处理 add；或至少在注释与辅助函数上与 L4 命名一致），减少「合并表 + 先插后删」与「分阶段」两套叙事并存。若保留合并表，应在同一函数内用**明确子步骤**（注释或抽取函数）标出等价的 remove/update/add 语义，避免与 TLS 历史实现混淆。

**TLS 的优先动作：** 将 `partial_update` 中三循环顺序改为与 `tcp/conf_handler_impl.rs` / `udp/conf_handler_impl.rs` 一致，并补充**同批次多操作**的回归测试（例如同时 add、update、remove 不同键，以及 update 导致 listener 迁移的情况），与 UDP 已有测试风格对齐。

---

## 可考虑抽象为共享逻辑的方案

在不对所有协议强行统一数据结构的前提下，可抽取**纯编排**层，降低重复与顺序错误：

1. **通用「增量编排」辅助函数（泛型或 trait）**  
   - 输入：`add`、`update`、`remove` 及回调：`fn collect_old_listeners(keys: impl Iterator<Item = &str>)`、`fn remove(key)`、`fn upsert(key, route)`（内部完成 initialize + insert，或 HTTP/gRPC 的 insert + 派生结构）。  
   - 固定调用顺序：`collect_old`（remove ∪ update）→ 循环 `remove` → 循环 `update` → 循环 `add` → `rebuild_affected(&listeners)`。  
   - HTTP/gRPC 将「upsert」实现为写 `route_cache`（及 gRPC 的 units），「remove」实现为删缓存；**收集 affected** 仍可复用「旧 + 新」两套 listener 逻辑。

2. **文档化宏或 `partial_update_l4!` 最小模板**  
   - 仅用于 TLS/TCP/UDP 三者结构极度相似的路径，避免宏滥用；或用一个私有 `fn apply_l4_partial_update(...)` 放在 `routes` 下公共模块，三处只传 `initialize_route` 与类型相关闭包。

3. **不变式与注释**  
   - 在 `ConfHandler` 实现处增加简短模块级注释：**partial_update 必须遵循 remove → update → add，且 affected listeners 须覆盖旧绑定与新绑定**。  
   - 若某协议因性能必须偏离，需在代码审查中要求单独 ADR 或注释说明理由。

---

## 小结

- 五类协议的 `partial_update` 在 **L4 内部** 已分裂为 **TCP/UDP** 与 **TLS** 两种相反顺序；**L7（HTTP/gRPC）** 又采用 **合并 add/update + 缓存先批量写入、remove 在后** 的第三种形态。  
- **建议统一以 remove → update → add 为规范**，并**优先修正 TLS** 与 TCP/UDP 的对齐；HTTP/gRPC 可通过等价步骤划分或共享编排函数与文档，减少三套心智模型并存。  
- 长期可通过**小型共享编排层**固化顺序，把「顺序正确」从人肉 Review 转为单点实现。
