# [P3] PolicyStore 的 remove_by_route 中不必要的 clone 开销

## 问题描述

`PolicyStore::remove_by_route` 在 `DashMap::retain` 的回调里对每条 `ServiceLbPolicy` 执行 `policy.clone()`，再在可能保留条目时通过 `*policy = entry` 写回。`retain` 对值的回调签名中 `policy` 已是 `&mut ServiceLbPolicy`，可直接原地修改，当前实现引入了与 store 规模成正比的深拷贝开销。

此外，`batch_remove_routes` 对每个待删路由重复调用 `remove_by_route`，导致对整张表的多次全量扫描；`add_policy` 返回的 `changed` 标志仅依据 `policies` 向量长度是否增长，未覆盖「仅 `route_refs` 变化」的语义，可能与调用方对「是否有变更」的预期不一致。

## 影响范围

- **模块**：`edgion` 网关负载均衡策略存储，`src/core/gateway/lb/lb_policy/policy_store.rs`。
- **调用链**：任何通过 `remove_by_route`、`batch_remove_routes` 或 `delete_lb_policies_by_resource_key` 在路由删除/同步时清理 LB 策略引用的路径；`batch_add` 依赖 `add_policy` 的返回值触发 endpoint slice 更新事件。
- **典型场景**：集群中存在较多带 LB 策略的 `Service` 条目、或单次批量删除多条 HTTPRoute 引用时，CPU 与分配压力更明显。

## 严重程度

| 维度 | 说明 |
|------|------|
| **级别** | **P3**（性能与可维护性；非功能性正确性错误） |
| **正确性** | 现有 clone/写回逻辑在语义上与「原地修改」等价，一般不改变行为。 |
| **风险** | 高基数 service × 频繁路由变更时放大 GC 与缓存压力；`changed` 语义偏差可能导致**应触发而未触发**下游更新（若业务依赖该布尔值）。 |

## 代码位置

| 项 | 文件 | 大致行号 |
|----|------|----------|
| `ServiceLbPolicy` 结构（含 `Vec<LbPolicy>`、`HashSet<String>`） | `policy_store.rs` | 22–29 |
| `add_policy` 与 `changed` 判定 | `policy_store.rs` | 110–141 |
| `remove_by_route`（clone + `removed_services`） | `policy_store.rs` | 195–225 |
| `batch_remove_routes` | `policy_store.rs` | 231–235 |

## 详细分析

### 1. `remove_by_route` 中不必要的 clone

当前实现：

```199:210:/Users/caohao/ws1/Edgion/src/core/gateway/lb/lb_policy/policy_store.rs
        self.policies.retain(|service_key, policy| {
            let mut entry = policy.clone();
            let should_delete = entry.remove_route_ref(route_key);

            if should_delete {
                removed_services.push(service_key.clone());
                false // Remove from map
            } else {
                // Update the entry with modified route_refs
                *policy = entry;
                true // Keep in map
            }
        });
```

`DashMap::retain` 对值的闭包参数 `policy` 为 `&mut V`（此处即 `&mut ServiceLbPolicy`）。`ServiceLbPolicy::remove_route_ref` 仅需 `&mut self`，可直接对 `policy` 调用，无需临时拥有权。先 `clone` 再写回会使**每个被遍历到的条目**至少发生一次对 `Vec<LbPolicy>` 与 `HashSet<String>` 的深拷贝；保留分支上还存在一次整体赋值，属于冗余工作。

### 2. 每次 `retain` 迭代 clone 的放大效应

`ServiceLbPolicy` 为 `#[derive(Clone)]`，clone 会复制全部策略列表与全部路由引用键。一次 `remove_by_route` 会扫描 map 中**所有** service 条目；若条目数为数百且每条策略/引用较多，单次删除路由即可产生 O(M) 次大对象 clone（M = 当前 map 大小），与「仅修改一个 `HashSet` 的一次 `remove`」所需成本不匹配。

### 3. `removed_services` 仅用于日志

`removed_services` 在删除分支收集 `service_key`，最终仅用于 `tracing::info`/`debug`：

```213:224:/Users/caohao/ws1/Edgion/src/core/gateway/lb/lb_policy/policy_store.rs
        if !removed_services.is_empty() {
            tracing::info!(
                route_key = %route_key,
                removed_services = ?removed_services,
                "Removed LB policies for route"
            );
        } else {
            tracing::debug!(
                route_key = %route_key,
                "No LB policies to remove for route"
            );
        }
```

这带来额外 `Vec` 分配与若干 `String` clone。若可接受日志粒度变化，可在 `should_delete` 为真时在回调内直接 `tracing::info!`（或先计数、最后一条汇总日志），避免为日志单独维护集合。（注意：在并发 map 的回调内打日志需符合团队对日志量与锁竞争的约束。）

### 4. `batch_remove_routes` 串行全表扫描

```231:235:/Users/caohao/ws1/Edgion/src/core/gateway/lb/lb_policy/policy_store.rs
    pub fn batch_remove_routes(&self, route_keys: &[String]) {
        for route_key in route_keys {
            self.remove_by_route(route_key);
        }
    }
```

每轮 `remove_by_route` 均执行一次完整 `retain`（全表遍历）。设待删路由数为 N、当前 service 条目数为 M，则共执行 **N 次** 全表扫描，仅遍历次数即为 **O(N × M)**；若每次回调仍含 clone，额外成本再乘在每条目上。优化为单次 `retain` 后，对 map 仅遍历 **O(M)** 次；每条目上对 N 个路由键各做一次 `HashSet::remove`，总移除操作为 **O(M × N)**，但避免了 **N−1** 次重复的 map 扫描与（优化前）重复的整表 clone。

### 5. `add_policy` 中 `changed` 的语义缺口

```117:131:/Users/caohao/ws1/Edgion/src/core/gateway/lb/lb_policy/policy_store.rs
        self.policies
            .entry(service_key.clone())
            .and_modify(|entry| {
                let old_policies_len = entry.policies.len();
                entry.add_route_ref(route_key.clone(), policies.clone());
                // Check if any new policies were added
                if entry.policies.len() > old_policies_len {
                    changed = true;
                }
            })
            .or_insert_with(|| {
                // New service key, this is a change
                changed = true;
                ServiceLbPolicy::new(policies.clone(), route_key.clone())
            });
```

`add_route_ref` 会先 `route_refs.insert(route_key)`，再合并策略；若 **`route_key` 为新** 但传入的 `new_policies` 与已有策略完全重复（`Vec` 长度不变），则 `changed` 仍为 `false`，但 store 的引用关系已更新（`route_refs` 增加）。反之若 `route_key` 已存在，`HashSet::insert` 不增加元素，但合并逻辑仍可能添加新策略；当前用 `policies.len()` 判断可覆盖「新策略」情况。

闭包对外部 `changed` 的修改在 Rust 中合法（`FnMut` 捕获 `&mut changed`），问题不在借用检查，而在**文档与返回值语义**：注释写「new key or new policies added」，实现未包含「新 route 引用、策略集合未变」的情形。`batch_add` 根据 `changed` 触发 `trigger_endpoint_slice_update_event`，若产品语义要求「任意 route 引用变化都应刷新」，当前实现可能漏触发。

## 修改建议

1. **`remove_by_route`**：在 `retain` 内直接 `policy.remove_route_ref(route_key)`，去掉 `clone` 与 `*policy = entry`。
2. **`batch_remove_routes`**：改为单次 `retain`（或等价单次遍历），对每个 `policy` 依次对 `route_keys` 执行 `route_refs.remove`（或新增 `ServiceLbPolicy` 上的批量方法），最后仅根据 `route_refs.is_empty()` 决定是否移除条目。
3. **日志**：评估在删除分支内记录 `service_key`、或仅记录删除数量，以去掉 `removed_services` 向量；若需保留完整列表且规模大，可改为 `tracing` 的结构化字段或限频。
4. **`add_policy` / `changed`**：在模块或 API 注释中明确 `changed` 定义；若需与 endpoint 刷新对齐，可在 `and_modify` 中根据 `route_refs.insert` 的返回值、`policies` 长度变化等组合判定，或拆分为「引用变化」与「策略列表变化」两个标志供调用方使用。

## 优化后代码示例

以下示例仅作说明，命名与日志策略可按项目规范微调。

### `remove_by_route`：去掉 clone，可选内联日志

```rust
pub fn remove_by_route(&self, route_key: &str) {
    let mut any_removed = false;

    self.policies.retain(|service_key, policy| {
        let should_delete = policy.remove_route_ref(route_key);
        if should_delete {
            any_removed = true;
            tracing::info!(
                route_key = %route_key,
                service_key = %service_key,
                "Removed LB policy entry (last route ref)"
            );
            false
        } else {
            true
        }
    });

    if !any_removed {
        tracing::debug!(
            route_key = %route_key,
            "No LB policies to remove for route"
        );
    }
}
```

若必须保留「单次 info 输出所有 `removed_services`」的行为，可在无 clone 的前提下仍用 `Vec` 收集，但仅在实际发生删除时分配，且条目数通常远小于「每次 retain 全量 clone」的成本。

### `batch_remove_routes`：单次遍历

`ServiceLbPolicy` 的 `route_refs` 为私有字段，宜新增批量方法（与 `remove_route_ref` 语义一致），再在 store 层单次 `retain`：

```rust
impl ServiceLbPolicy {
    /// 从 route_refs 中移除多个路由键；若已无引用则返回 true（应删除整条目）
    pub fn remove_route_refs(&mut self, route_keys: &[String]) -> bool {
        for rk in route_keys {
            self.route_refs.remove(rk.as_str());
        }
        self.route_refs.is_empty()
    }
}

pub fn batch_remove_routes(&self, route_keys: &[String]) {
    let mut removed_services = Vec::new();

    self.policies.retain(|service_key, policy| {
        if policy.remove_route_refs(route_keys) {
            removed_services.push(service_key.clone());
            false
        } else {
            true
        }
    });

    if !removed_services.is_empty() {
        tracing::info!(
            route_keys = ?route_keys,
            removed_services = ?removed_services,
            "Batch removed LB policies for routes"
        );
    }
}
```

### `add_policy`：`changed` 覆盖新 route 引用

```rust
// 在 and_modify 分支中，根据是否新增引用或是否新增策略共同决定
.and_modify(|entry| {
    let old_policies_len = entry.policies.len();
    let was_new_ref = entry.route_refs.insert(route_key.clone());
    for policy in policies.clone() {
        if !entry.policies.contains(&policy) {
            entry.policies.push(policy);
        }
    }
    let policies_grew = entry.policies.len() > old_policies_len;
    if was_new_ref || policies_grew {
        changed = true;
    }
})
```

注意：`add_route_ref` 当前把 `insert` 与合并封装在一起；若采用上述展开，应与 `add_route_ref` 语义保持一致或重构为单一实现路径，避免重复逻辑。

---

**结论**：优先移除 `remove_by_route` 中的 clone 与合并 `batch_remove_routes` 为单次扫描，可显著降低热点路径上的分配与 CPU；`changed` 语义建议与 `batch_add` 触发更新的产品需求对齐后一次性修正并补充文档。
