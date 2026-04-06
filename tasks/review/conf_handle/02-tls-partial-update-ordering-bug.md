# TLS `partial_update` 处理顺序与受影响 Listener 收集不完整

## 严重程度

**高**

## 影响范围

- **模块**：`GlobalTlsRouteManagers::partial_update`（`src/core/gateway/routes/tls/conf_handler_impl.rs`）
- **行为**：TLS 路由增量同步时，路由缓存与按 Listener 重建的路由表之间可能出现短暂不一致；在「同一次 partial 中同时存在 add 与 remove（或等价场景）」时，问题更明显。
- **用户可见影响**：同一 TLS Listener 上 SNI/主机名匹配可能出现短暂错误路由、重复或冲突感知；若上游将本应为 **update** 的事件误标为 **add**，旧 Listener 上的路由表可能**长期不刷新**（脏数据）。
- **对比**：TCP/UDP 同类逻辑已采用 **remove → update → add**；HTTP 采用合并缓存后统一重建，规避了此类顺序问题。

---

## 问题详细描述

### 当前 TLS 实现的处理顺序

`partial_update` 中，在收集「待重建」的 `ListenerKey` 之后，对路由的实际变更顺序为：

1. **add**：先插入新路由  
2. **update**：删旧再插新  
3. **remove**：最后删除  

对应代码如下：

```66:119:/Users/caohao/ws1/Edgion/src/core/gateway/routes/tls/conf_handler_impl.rs
        let mut affected_listeners: HashSet<ListenerKey> = HashSet::new();

        // Collect old listeners from routes being removed or updated BEFORE modifying the cache
        for key in remove.iter().chain(update.keys()) {
            if let Some(entry) = self.get_route(key) {
                for lk in resolved_listeners_for_route(&entry) {
                    affected_listeners.insert(lk.clone());
                }
            }
        }

        for (key, route) in add {
            match self.initialize_route(route) {
                Ok(initialized) => {
                    for lk in resolved_listeners_for_route(&initialized) {
                        affected_listeners.insert(lk.clone());
                    }
                    self.insert_route(initialized);
                    // ...
                }
                // ...
            }
        }

        for (key, route) in update {
            self.remove_route(&key);
            // ... insert new ...
        }

        for key in remove {
            self.remove_route(&key);
            // ...
        }
```

### Bug 1：`add` 先于 `remove` 执行

**现象**：同一次 `partial_update` 调用中，若 **add** 与 **remove** 涉及**同一逻辑 Listener**（或会在同一张路由表里产生重叠 SNI/主机名语义），先执行 **add** 再执行 **remove** 会使缓存中在短时间内同时存在「待删的旧路由」与「新来的路由」，再经 `rebuild_affected_listener_managers` 汇总——虽然最终在**一次重建**后可能收敛到正确表，但：

- 中间态下 `insert_route` / 内部索引若按「追加」语义处理，可能与「应先腾出再写入」的期望不一致；
- 与 TCP/UDP 已采用的 **先删、再改、再加** 不一致，增加与其他层（例如假设删除优先）的逻辑分歧与排查成本。

**TCP 正确顺序（对照）**：

```79:124:/Users/caohao/ws1/Edgion/src/core/gateway/routes/tcp/conf_handler_impl.rs
        // Process removals
        for key in &remove {
            self.remove_route(key);
            tracing::debug!(resource_key = %key, "Removed TCPRoute");
        }

        // Process updates (remove old, add new)
        for (key, route) in update {
            self.remove_route(&key);
            // ...
        }

        // Process additions
        for (key, route) in add {
            // ...
        }
```

**UDP** 与 TCP 结构相同（`src/core/gateway/routes/udp/conf_handler_impl.rs` 第 79–124 行）：**remove → update → add**。

### Bug 2：预收集阶段未包含 `add` 键对应的「旧路由」Listener

预收集仅遍历：

```69:69:/Users/caohao/ws1/Edgion/src/core/gateway/routes/tls/conf_handler_impl.rs
        for key in remove.iter().chain(update.keys()) {
```

**未包含** `add.keys()`。

若某资源键 `K` 在缓存中**已存在**（例如控制器/同步层误将 **update** 放进 **add**），则：

- 预收集阶段**不会**读取 `get_route(K)` 的旧版本，旧版本所绑定的 `ListenerKey` **不会**进入 `affected_listeners`；
- 随后 `add` 分支对 `K` 执行 `insert_route`，可能覆盖缓存中的路由；`add` 循环里只会把**新**路由解析出的 Listener 加入 `affected_listeners`；
- 若旧路由与新版解析到的 **Listener 不同**（例如 parentRefs/port 变更），**旧 Listener 上的路由表可能不会被重建**，形成**长期脏数据**。

HTTP 路径在 `partial_update` 中对 **add/update 合并后的每个 key** 会同时合并「新路由的 listeners」与「缓存中旧路由的 listeners」：

```177:194:/Users/caohao/ws1/Edgion/src/core/gateway/routes/http/conf_handler_impl.rs
        for (key, route) in &add_or_update {
            let listeners = resolved_listeners_for_route(route);
            // ... insert new-side listeners ...
            if let Some(old) = self.route_cache.get(key) {
                for lk in resolved_listeners_for_route(old.value()) {
                    affected_listeners.insert(lk.clone());
                }
            }
        }
```

TLS 当前缺少对 **`add` 键** 的「旧侧」Listener 收集，与 HTTP 的稳健性不一致。

---

## 根因分析

1. **顺序**：TLS 的 `partial_update` 在演进时未与 TCP/UDP 对齐为「删除优先、更新其次、新增最后」。在共享 Listener、SNI 主机名竞争或内部去重逻辑存在的前提下，**先加后删**会破坏「旧条目应先退出再接纳新条目」这一常见不变量。

2. **受影响 Listener 集合**：`affected_listeners` 的设计意图是「凡路由变更波及的 Listener 都要重建」。但预收集只覆盖 `remove` 与 `update`，**隐含假设** `add` 的 key 在缓存中不存在。该假设在配置客户端分类错误、重放、或合并批次时可能被打破；HTTP 已通过「对 add_or_update 的每个 key 读取旧缓存」显式消除该假设。

3. **一致性模型**：TLS 与 TCP/UDP 一样采用「改缓存 + 最后 `rebuild_affected_listener_managers`」；顺序与集合完整性直接影响单次调用内缓存与重建集合是否与实际变更闭环一致。

---

## 修复建议（含具体代码）

### 1. 与 TCP/UDP 对齐处理顺序：remove → update → add

在 `GlobalTlsRouteManagers::partial_update` 中，将三个循环调整为与 `conf_handler_impl.rs`（TCP）相同的顺序：

- 先 **`for key in &remove { self.remove_route(key); }`**
- 再 **update**（`remove_route` + `initialize_route` + `insert_route`）
- 最后 **add**

保持首部「预收集」仍在**修改缓存之前**执行（见下节对预收集的增强）。

### 2. 预收集阶段纳入 `add` 键的旧路由 Listener

在修改缓存前，将遍历扩展为例如：

```rust
for key in remove
    .iter()
    .chain(update.keys())
    .chain(add.keys())
{
    if let Some(entry) = self.get_route(key) {
        for lk in resolved_listeners_for_route(&entry) {
            affected_listeners.insert(lk.clone());
        }
    }
}
```

这样即使 `add` 误承载「对已存在 key 的覆盖」，旧 Listener 仍会进入重建集合。

### 3. 参考补丁结构（逻辑示意，非逐字粘贴）

```rust
fn partial_update(
    &self,
    add: HashMap<String, TLSRoute>,
    update: HashMap<String, TLSRoute>,
    remove: HashSet<String>,
) {
    // ... tracing ...

    let mut affected_listeners: HashSet<ListenerKey> = HashSet::new();

    for key in remove
        .iter()
        .chain(update.keys())
        .chain(add.keys())
    {
        if let Some(entry) = self.get_route(key) {
            for lk in resolved_listeners_for_route(&entry) {
                affected_listeners.insert(lk.clone());
            }
        }
    }

    for key in &remove {
        self.remove_route(key);
        tracing::debug!(resource_key = %key, "Removed TLSRoute");
    }

    for (key, route) in update {
        self.remove_route(&key);
        match self.initialize_route(route) {
            Ok(initialized) => {
                for lk in resolved_listeners_for_route(&initialized) {
                    affected_listeners.insert(lk.clone());
                }
                self.insert_route(initialized);
                tracing::debug!(resource_key = %key, "Updated TLSRoute");
            }
            Err(e) => {
                tracing::error!(/* ... */);
            }
        }
    }

    for (key, route) in add {
        match self.initialize_route(route) {
            Ok(initialized) => {
                for lk in resolved_listeners_for_route(&initialized) {
                    affected_listeners.insert(lk.clone());
                }
                self.insert_route(initialized);
                tracing::debug!(resource_key = %key, "Added TLSRoute");
            }
            Err(e) => {
                tracing::error!(/* ... */);
            }
        }
    }

    self.rebuild_affected_listener_managers(&affected_listeners);
}
```

**注意**：若 `remove` 与 `add`/`update` 在同一批次中对同一 `key` 同时出现，应遵循配置上游的契约（通常不应发生）。若存在，需在合并策略或此处增加显式优先级与告警日志；本修复以与 TCP/UDP 一致为先。

### 4. 可选的长期方案

若希望与 HTTP 一样减少「多阶段顺序」类缺陷，可考虑 TLS 也采用「先合并 add/update，再批量更新缓存，最后按受影响 Listener 重建」的模式；改动面较大，需评估与 `insert_route`/`remove_route` 副作用的一致性。

---

## 与其他协议的对比分析

| 维度 | TLS（当前） | TCP / UDP | HTTP |
|------|-------------|-----------|------|
| 变更顺序 | add → update → remove | remove → update → add | 先合并 add+update，再删缓存项，再一次重建 |
| 预收集是否含 add 的 key | 否 | 否 | 是（对 `add_or_update` 每个 key 读旧缓存） |
| 顺序风险 | 高（与流式协议栈不一致） | 低 | 低（缓存更新与 listener 集合计算方式不同） |
| add 误分类为 update 的健壮性 | 差（缺旧 Listener） | 差（同样未 chain add keys） | 好 |

**说明**：TCP/UDP 与 TLS 在「预收集不含 `add.keys()`」上当前一致；用户报告主要强调 **TLS 顺序错误** 及 **add/remove 交织** 的风险。建议在修复 TLS 时**同步**为 TCP/UDP 增加 `chain(add.keys())`，三端行为一致，避免仅修 TLS 后 TCP/UDP 仍存同类「误 add」隐患。

---

## 测试建议

1. **同批 add + remove（同一 Listener）**  
   - 初始：`full_set` 或 `partial_update` 写入 routeA（listener L，主机名 H）。  
   - 一次 `partial_update`：`remove` 含 routeA，`add` 含 routeB（同 L、同 H 或互斥 SNI 场景按产品语义设计断言）。  
   - 断言：重建后 `load_route_table()` 仅反映最终应存在的一条路由，且过程中若可观测中间态，应符合「先删后加」的预期（必要时在测试中通过 mock 或统计 `insert_route` 调用顺序验证）。

2. **误分类：已存在 key 走 add**  
   - 缓存中已有 `default/r1` 绑定 Listener L1。  
   - `partial_update` 仅带 `add["default/r1"]`，且新规格解析到 L2。  
   - 断言：L1、L2 的 manager 均触发重建；L1 上不再残留旧 r1。

3. **对齐 UDP 已有用例**  
   - 可参考 `test_udp_route_partial_update_concurrent_add_and_remove`（`udp/conf_handler_impl.rs`），为 TLS 编写平行测试，确保 **remove → update → add** 下行为与 UDP 一致。

4. **回归**  
   - 运行现有 `conf_handler_impl.rs` 内 TLS 单测（如 `test_tls_route_partial_update`），并补充上述场景后执行 `cargo test` 相关模块。

---

## 小结

TLS `partial_update` 存在 **（1）add 在 remove 之前执行** 与 **（2）预收集未包含 add 键对应旧路由的 Listener** 两类问题，严重程度 **高**，可能导致 Listener 级路由表短暂或长期不一致。建议将处理顺序与 TCP/UDP 对齐，并在预收集阶段 **`chain(add.keys())`**（并建议三协议统一），必要时增加针对「同批 add/remove」与「add 覆盖已存在 key」的集成测试。
