# 问题标题

**Endpoints 与 EndpointSlice 配置处理器中 `cleanup_removed_backends` 及控制流高度重复**

---

## 元信息

| 项 | 内容 |
|----|------|
| **严重程度** | 中 |
| **涉及文件** | `src/core/gateway/backends/discovery/endpoint/conf_handler_impl.rs` |
| | `src/core/gateway/backends/discovery/endpoint_slice/conf_handler_impl.rs` |
| **审查焦点** | 代码重复、DRY 违反、可维护性与重构方向 |

---

## 背景与结论摘要

`EndpointHandler` 与 `EpSliceHandler` 在架构上遵循同一套设计（仅 RoundRobin 存数据 + LB，其他策略从 RR 读后端）。实现上，除「健康检查注解解析」与「配置存储 API（Endpoints vs EndpointSlice）」外，大量逻辑为逐行等价或仅差函数名/类型名。其中 **`cleanup_removed_backends` 两文件内函数体完全一致**；`update_all_existing_lbs`、`update_affected_lbs`、`full_set`/`partial_update` 的主干流程也几乎相同。

**风险**：任一处的行为修正（例如清理顺序、缓存失效条件、熔断器清理）若只改一侧，易产生 **双实现漂移**；新同事阅读时需维护两份心智模型。

---

## 具体重复代码的逐行对比

### 1. `cleanup_removed_backends`（完全重复）

**Endpoint 版**（`endpoint/conf_handler_impl.rs` 第 11–35 行）与 **EndpointSlice 版**（`endpoint_slice/conf_handler_impl.rs` 第 13–37 行）对比如下。

| 行序（语义行） | Endpoint 文件 | EndpointSlice 文件 | 是否一致 |
|----------------|---------------|-------------------|----------|
| 文档注释 | 无 | `/// Diff old vs new...` | Slice 多一行文档，逻辑无差异 |
| 函数签名 | `fn cleanup_removed_backends(service_key: &str, old: &HashSet<SocketAddr>, new: &HashSet<SocketAddr>)` | 同左 | ✅ 一致 |
| `removed` | `old.difference(new).cloned().collect()` | 同左 | ✅ 一致 |
| `added` | `new.difference(old).cloned().collect()` | 同左 | ✅ 一致（**两侧均未使用 `added`，属重复的死代码**） |
| 有增删则失效 selector | `if !removed.is_empty() \|\| !added.is_empty() { invalidate_selector_cache(...) }` | 同左 | ✅ 一致 |
| `removed` 非空分支 | `tracing::debug!(...)` + `get_circuit_breaker_store()` + 循环 | 同左 | ✅ 一致 |
| 循环体内 | `get_count` → `mark_backend_draining` / `remove_backend` + `cb_store.remove_backend` | 同左 | ✅ 一致 |

**结论**：除 Slice 文件多了一段 `///` 注释外，**可视为字节级等价实现**，属于应合并的重复。

---

### 2. `update_all_existing_lbs`

| 对比项 | Endpoint | EndpointSlice |
|--------|----------|---------------|
| 取 store | `get_endpoint_roundrobin_store()` | `get_roundrobin_store()` |
| 循环体 | `for service_key in roundrobin_store.get_existing_service_keys()` → `update_lb_if_exists(&service_key)` | 同结构 | ✅ 逻辑等价 |

---

### 3. `update_affected_lbs`

| 对比项 | Endpoint | EndpointSlice |
|--------|----------|---------------|
| 取 store | `get_endpoint_roundrobin_store()` | `get_roundrobin_store()` |
| 对 `affected_services` 中每个 `service_key` | 从 LB 取 `old_addrs` → `update_lb_if_exists` → 从 store 取 `new_addrs` → `cleanup_removed_backends` | 同左 | ✅ 逻辑等价 |

---

### 4. `sync_health_check_configs_for_services`（结构重复，细节不同）

| 对比项 | Endpoint | EndpointSlice |
|--------|----------|---------------|
| 外层 | `get_hc_config_store()` + `get_health_check_manager()` + `for service_key in service_keys` | 同左 | ✅ 结构一致 |
| 解析配置 | `resolve_endpoint_config_for_service`（单 Endpoint + `parse_health_check_annotation`） | `resolve_endpoint_slice_config_for_service`（多 Slice、冲突则 `None`） | ⚠️ **业务不同**，不宜简单合并为同一函数体 |
| 写入 store | `config_store.set_endpoint_config(...)` | `config_store.set_endpoint_slice_config(...)` | ⚠️ API 不同 |
| reconcile | `hc_manager.reconcile_service(service_key)` | 同左 | ✅ 一致 |

---

### 5. `full_set`（主干重复，与配置源相关的调用点不同）

以下步骤在两文件中 **顺序与注释编号一致**：

1. `tracing::info!`（仅 `component` 字段不同：`endpoint_handler` vs `ep_slice_handler`）
2. `old_keys`：`config_store.endpoint_keys()` vs `endpoint_slice_keys()`
3. `replace_data_only(data.clone())`（类型参数：`HashMap<String, Endpoints>` vs `HashMap<String, EndpointSlice>`）
4. `update_all_existing_lbs()`
5. `sync_health_check_configs_for_services(&all_services)`
6. 对所有 `service_key`：`runtime_state::invalidate_selector_cache`
7. `stale_services` 计算与清理循环：  
   - `set_endpoint_config(service_key, None)` vs `set_endpoint_slice_config(service_key, None)`  
   - 其余：`reconcile_service`、`remove_service`、`cb_store.remove_service` 一致
8. 收尾 `tracing::info!`（`component` 与计数语义相同）

---

### 6. `partial_update`（主干重复）

1. `update_data_only(add, update, &remove)`（泛型在 `ConfHandler` 实现上不同）
2. `update_affected_lbs(&affected_services)`
3. `sync_health_check_configs_for_services(&affected_services)`
4. `tracing::info!`（`component` 不同）

---

### 7. 粗粒度「相似度」说明

若将「解析健康检查 + 写入 HC store 的分支」视为 **源差异**，其余 RoundRobin 更新、运行时清理、`full_set`/`partial_update` 流程在结构上高度重合，**主观估计两文件在控制流与样板代码层面约 70%–85% 同源**（与「约 80%」描述一致，具体比例取决于是否计入注释与 tracing 字段）。

---

## DRY 原则违反分析

1. **单一事实来源（SSOT）被破坏**  
   `cleanup_removed_backends` 描述「后端集合 diff → selector 缓存失效 → 熔断与 runtime_state 清理」的 **唯一正确行为** 应只存在一处。双份拷贝意味着 bugfix 或行为调整需要人工保证两处同步，易遗漏。

2. **重复即缺陷放大器**  
   两侧同时存在未使用的 `added` 变量：若未来要在「仅新增后端」时做优化或不同失效策略，开发者可能只改一侧，或在一侧删除 `added` 后另一侧仍保留，增加噪音与审查成本。

3. **可测试性**  
   游离在 `impl` 旁的自由函数若抽到公共模块，可单独写单元测试覆盖 diff 与清理分支；当前重复使「是否两边都测到」依赖集成路径。

4. **合理的不 DRY 边界**  
   `resolve_endpoint_config_for_service` 与 `resolve_endpoint_slice_config_for_service` **语义不同**（Slice 多 slice 冲突处理），**不应**为追求行数减少而强行揉成一个大函数；DRY 应止于「相同的后端生命周期与 store 更新管线」。

---

## 重构建议（提取共享模块 / trait / 函数）

### 建议 A：最低成本 — 提取公共自由函数（优先做）

- 将 `cleanup_removed_backends` 移到两模块的公共父级，例如：  
  `discovery/mod.rs` 下新建 `conf_handler_common.rs`，或 `discovery/common.rs`。
- **签名保持不变**，两 handler 仅 `use` 该函数。
- 顺带删除两侧未使用的 `added`，或若产品意图是「将来对 added 做处理」，应 **要么使用要么删除并加注释说明**。

**优点**：改动面小、无泛型复杂度、立刻消除完全重复。  
**缺点**：`full_set`/`partial_update` 仍有一定重复。

### 建议 B：中等成本 — 抽象「RoundRobin 数据源」访问

- 定义 **trait**，封装「拿 RR store、replace/update data、get_existing_service_keys」等与具体 K8s 类型无关、但与 store 类型相关的操作。
- 两个 handler 只实现「如何解析 HC 配置」与「old_keys / set_*_config」的薄适配层。

适合在 A 落地后，若 `full_set` 仍频繁双改，再引入。

### 建议 C：结构化重复 — 模板方法式辅助

- 将 `full_set`/`partial_update` 的共同步骤抽成 **接受闭包或 trait object** 的函数，例如：

  - `fn full_set_common<FOldKeys, FReplace, FSetNone>(...)`
  - 其中 `FOldKeys` 取旧 key 集合，`FReplace` 调用对应 `replace_data_only`，`FSetNone` 清理 stale 时写 `None`。

- 避免过度泛型化导致编译错误信息难读；可优先用 **小 struct + 方法** 而非长泛型参数列表。

---

## 泛型化方案示例代码

以下示例仅说明思路，**非可直接粘贴的完整工程代码**（需与现有 `get_*_store`、具体 store trait 对齐）。

### 示例 1：仅抽取 `cleanup_removed_backends`（推荐第一步）

```rust
// 例如：src/core/gateway/backends/discovery/conf_cleanup.rs
use pingora_core::protocols::l4::socket::SocketAddr;
use std::collections::HashSet;

/// 后端地址集合从 `old` 变为 `new` 时，失效 selector 缓存并清理已移除后端的运行时状态与熔断数据。
pub fn cleanup_removed_backends(
    service_key: &str,
    old: &HashSet<SocketAddr>,
    new: &HashSet<SocketAddr>,
) {
    let removed: Vec<SocketAddr> = old.difference(new).cloned().collect();
    // 与原逻辑 `!removed.is_empty() || !added.is_empty()` 等价：集合有任意差异即失效缓存
    if old != new {
        crate::core::gateway::lb::runtime_state::invalidate_selector_cache(service_key);
    }

    if !removed.is_empty() {
        tracing::debug!(
            service_key = %service_key,
            removed_count = removed.len(),
            "Cleaning stale backend runtime state"
        );
        let cb_store = crate::core::gateway::backends::get_circuit_breaker_store();
        for addr in &removed {
            if crate::core::gateway::lb::runtime_state::get_count(service_key, addr) > 0 {
                crate::core::gateway::lb::runtime_state::mark_backend_draining(service_key, addr);
            } else {
                crate::core::gateway::lb::runtime_state::remove_backend(service_key, addr);
            }
            cb_store.remove_backend(service_key, addr);
        }
    }
}
```

说明：用 `old != new` 替代「计算 `added` 再判空」与原文条件等价，且去掉未使用变量；若团队更偏好显式 diff，也可写 `let added: Vec<_> = new.difference(old).cloned().collect();` 并在条件中使用 `!added.is_empty()`。

### 示例 2：trait 区分「Endpoints / EndpointSlice」配置侧差异

```rust
use std::collections::{HashMap, HashSet};

pub trait EndpointDiscoveryHealthSink {
    fn old_config_keys(&self) -> HashSet<String>;
    fn clear_service_config(&self, service_key: &str);
}

pub struct EndpointHealthSink; // 持有或静态访问 get_hc_config_store
impl EndpointDiscoveryHealthSink for EndpointHealthSink {
    fn old_config_keys(&self) -> HashSet<String> {
        get_hc_config_store().endpoint_keys().into_iter().collect()
    }
    fn clear_service_config(&self, service_key: &str) {
        get_hc_config_store().set_endpoint_config(service_key, None);
    }
}

pub struct EndpointSliceHealthSink;
impl EndpointDiscoveryHealthSink for EndpointSliceHealthSink {
    fn old_config_keys(&self) -> HashSet<String> {
        get_hc_config_store().endpoint_slice_keys().into_iter().collect()
    }
    fn clear_service_config(&self, service_key: &str) {
        get_hc_config_store().set_endpoint_slice_config(service_key, None);
    }
}
```

`full_set` 的 stale 清理循环可只写一次，通过 `dyn EndpointDiscoveryHealthSink` 或泛型参数 `H: EndpointDiscoveryHealthSink` 注入差异点。

### 示例 3：泛型 `ConfHandler` 共用逻辑（示意）

```rust
pub trait RoundRobinDataStore<T> {
    fn replace_data_only(&self, data: HashMap<String, T>) -> HashSet<String>;
    fn update_data_only(
        &self,
        add: HashMap<String, T>,
        update: HashMap<String, T>,
        remove: &HashSet<String>,
    ) -> HashSet<String>;
    fn get_existing_service_keys(&self) -> impl Iterator<Item = String>;
    fn update_lb_if_exists(&self, service_key: &str);
    fn backends_from_lb_addrs(&self, service_key: &str) -> HashSet<SocketAddr>;
    fn backends_for_service_addrs(&self, service_key: &str) -> HashSet<SocketAddr>;
}

// 伪代码：具体 store 实现上述 trait 后，update_affected_lbs 可写成单一泛型函数
pub fn update_affected_lbs_for_store<S: RoundRobinDataStore<T>, T>(
    store: &S,
    affected_services: &HashSet<String>,
) {
    for service_key in affected_services {
        let old_addrs = store.backends_from_lb_addrs(service_key);
        store.update_lb_if_exists(service_key);
        let new_addrs = store.backends_for_service_addrs(service_key);
        cleanup_removed_backends(service_key, &old_addrs, &new_addrs);
    }
}
```

实际工程中更常见的做法是 **不为 `T` 抽象而抽象**，而是让 **具体的 EndpointRoundRobinStore / SliceRoundRobinStore 实现同一 trait**（关联类型或具体方法名 `get_backends_from_lb` 已在两 store 上存在时，trait 可直接 mirror 现有 API）。

---

## 落地顺序建议

1. **P0**：抽出 `cleanup_removed_backends` 到共享模块；统一处理未使用 `added`。  
2. **P1**：若 `full_set`/`partial_update` 仍反复双改，再引入 `EndpointDiscoveryHealthSink` 或等价小 trait，收敛 stale 清理与 old_keys 来源。  
3. **P2**：在 store 层统一 trait 后，再考虑 `update_affected_lbs` / `update_all_existing_lbs` 的完全泛型化，避免过早抽象。

---

## 审查结论

| 项 | 说明 |
|----|------|
| **确认** | `cleanup_removed_backends` 两文件实现一致，属明确重复。 |
| **严重程度** | **中**：当前行为正确时风险主要是维护性与一致性；若未来只修一侧，可能引入生产行为差异。 |
| **推荐行动** | 至少完成 **建议 A**；其余按迭代成本择优推进。 |
