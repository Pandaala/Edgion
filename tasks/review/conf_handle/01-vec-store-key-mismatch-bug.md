# 问题标题

**配置处理器：`Vec` 缓存与 `namespace/name` 资源键不一致，导致 `partial_update` 删除失效、更新歧义与静默丢数据**

---

## 严重程度

**严重 / 高**

- **严重**：在 conf_sync 使用 `namespace/name` 作为资源键的前提下，`remove` 分支无法从缓存中移除已删除资源，网关侧长期持有**幽灵配置**，行为与集群真实状态不一致。
- **高**：跨命名空间同名资源在 `Vec` + `name_any()` 匹配下存在**错误更新**风险；`update` 未命中时**无日志、无插入回退**，运维与排障困难。

---

## 影响范围

| 维度 | 说明 |
|------|------|
| **直接受影响模块** | `EdgionGatewayConfig` 与 `GatewayClass` 的全局内存缓存及其 `ConfHandler` 实现 |
| **受影响文件** | `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`、`src/core/gateway/config/gateway_class/conf_handler_impl.rs` |
| **上游契约** | `ConfHandler::partial_update` 的 `add`/`update` 为 `HashMap<String, T>`，`remove` 为 `HashSet<String>`；键语义由 conf_sync 层定义 |
| **下游行为** | 依赖 `get_*_by_name` / `list_*` 或该缓存一致性的网关逻辑：删除事件不生效、列表膨胀、可能读到错误命名空间下的同名资源版本 |
| **结构性相似代码** | 两文件为明显的复制粘贴结构，**需同步修复**，避免一类资源修好、另一类仍坏 |

**与 conf_sync 的一致性证据**：缓存客户端以 `resource.key_name()`（即 `namespace/name`）作为键处理事件并压缩后调用 `partial_update`：

```29:44:Edgion/src/core/gateway/conf_sync/cache_client/event_dispatch.rs
        let namespace = resource.namespace();
        // ...
        let resource_key = resource.key_name();
        // ...
                    cache.insert(resource_key, resource);
        // ...
                    cache.remove(&resource_key);
```

```331:340:Edgion/src/core/gateway/conf_sync/cache_client/event_dispatch.rs
                                                    let resource_key = resource.key_name(); // Use namespace/name as key
                                                            cache.insert(resource_key.clone(), resource);
                                                            cache.add_compress_event(resource_key, change);
```

因此传入各 `ConfHandler::partial_update` 的 `remove` 集合元素应与 **`key_name()` 格式一致**（通常为 `namespace/name`），而非裸资源名。

---

## 问题详细描述（含代码引用）

### 1. 使用 `Vec<T>` 做主存，按名线性扫描

全局静态存储为 `Arc<RwLock<Vec<...>>>`，查询接口按 `name_any()` 线性查找：

```7:22:Edgion/src/core/gateway/config/edgion_gateway/conf_handler_impl.rs
/// Global store for EdgionGatewayConfig resources
static EDGION_GATEWAY_CONFIG_STORE: std::sync::LazyLock<Arc<RwLock<Vec<EdgionGatewayConfig>>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(Vec::new())));

/// Query EdgionGatewayConfig by name
#[allow(dead_code)]
pub fn get_edgion_gateway_config_by_name(name: &str) -> Option<EdgionGatewayConfig> {
    let store = EDGION_GATEWAY_CONFIG_STORE.read().unwrap();
    store.iter().find(|config| config.name_any() == name).cloned()
}
```

`GatewayClass` 侧完全同构：

```7:22:Edgion/src/core/gateway/config/gateway_class/conf_handler_impl.rs
/// Global store for GatewayClass resources
static GATEWAY_CLASS_STORE: std::sync::LazyLock<Arc<RwLock<Vec<GatewayClass>>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(Vec::new())));

/// Query GatewayClass by name
#[allow(dead_code)]
pub fn get_gateway_class_by_name(name: &str) -> Option<GatewayClass> {
    let store = GATEWAY_CLASS_STORE.read().unwrap();
    store.iter().find(|gc| gc.name_any() == name).cloned()
}
```

**问题**：资源规模上升时增删查均为 **O(n)**；且 API 仅暴露「按裸名」查询，**无法表达命名空间**，与多租户 Kubernetes 模型不一致。

---

### 2. 【关键】`partial_update` 删除：`remove` 键与 `retain` 比较字段不一致

`remove` 中应为 `namespace/name`（与上游 `key_name()` 一致），但 `retain` 使用 `config.name_any()`（仅 `name` 字段）参与 `HashSet::contains`：

```105:119:Edgion/src/core/gateway/config/edgion_gateway/conf_handler_impl.rs
        if !remove.is_empty() {
            tracing::info!(
                count = remove.len(),
                "EdgionGatewayConfig partial_update: removed {} EdgionGatewayConfig resources",
                remove.len()
            );
            for key in &remove {
                tracing::info!(
                    key = %key,
                    "EdgionGatewayConfig removed"
                );
            }
            // Remove configs whose key is in the remove set
            store.retain(|config| !remove.contains(&config.name_any()));
        }
```

```105:119:Edgion/src/core/gateway/config/gateway_class/conf_handler_impl.rs
        if !remove.is_empty() {
            // ...
            // Remove gateway_classes whose key is in the remove set
            store.retain(|gc| !remove.contains(&gc.name_any()));
        }
```

**后果**：只要 `remove` 的元素是 `"default/my-config"` 这类字符串，而 `name_any()` 返回 `"my-config"`，则 **`contains` 恒为 false**，**缓存条目永远不会被删除**。日志仍会打印「removed N」，具有误导性。

---

### 3. `partial_update` 更新：仅用 `name_any()` 定位，跨命名空间同名冲突

`update` 分支遍历 `HashMap` 时虽有 `(key, config)`，但定位已有项时忽略 `key`，仅用裸名：

```86:102:Edgion/src/core/gateway/config/edgion_gateway/conf_handler_impl.rs
        if !update.is_empty() {
            // ...
            for (key, config) in update {
                tracing::info!(
                    key = %key,
                    name = %config.name_any(),
                    "EdgionGatewayConfig updated"
                );
                // Find and update existing config
                if let Some(existing) = store.iter_mut().find(|c| c.name_any() == config.name_any()) {
                    *existing = config;
                }
            }
        }
```

**后果**：若 `Vec` 中同时存在 `ns-a/foo` 与 `ns-b/foo`，`find` 返回**第一个匹配裸名**的项，可能把 **A 命名空间的缓存槽位错写成 B 的资源**，造成静默数据错乱。

---

### 4. 更新未命中时静默丢弃

当 `store.iter_mut().find(...)` 未找到任何项时，**不写入、不 `push`、不打 `warn`**，本次 `update` 中的资源被直接丢弃。结合第 2 点，若此前 `remove` 从未真正删掉旧项，`Vec` 内状态与集群的对应关系会进一步混乱。

---

### 5. `full_set` 丢弃 Map 的键，仅保留值

`full_set` 接收 `HashMap<String, T>`，但只把 `config` 推入 `Vec`，**未保存 `key` 与对象的对应关系**：

```41:59:Edgion/src/core/gateway/config/edgion_gateway/conf_handler_impl.rs
    fn full_set(&self, data: &HashMap<String, EdgionGatewayConfig>) {
        // ...
        let mut store = EDGION_GATEWAY_CONFIG_STORE.write().unwrap();
        store.clear();
        for (key, config) in data {
            tracing::debug!(
                key = %key,
                name = %config.name_any(),
                "EdgionGatewayConfig stored"
            );
            store.push(config.clone());
        }
    }
```

**后果**：后续所有逻辑只能依赖对象内的 `namespace()` / `name_any()` 等字段；若与上游 `key` 的编码规则出现偏差（例如未来键格式扩展），**无法在缓存层校验一致性**。根本问题仍是：**应以与 conf_sync 相同的字符串键作为索引**，而不是假设「裸名足够唯一」。

---

## 根因分析

1. **契约理解偏差**：`ConfHandler` 的 `String` 键在 conf_sync 路径上明确为 **`key_name()`（`namespace/name`）**，而实现侧在 `retain` / `find` 中改用 **`name_any()`（仅 name）**，导致集合成员与比较值**类型相同但语义不同**，属于典型的**键空间不一致**缺陷。

2. **数据结构选型不当**：用 `Vec` 表达「按资源键唯一」的集合，天然需要线性扫描；在已有 `HashMap` 入参（`full_set` / `partial_update`）的情况下，内层仍降为无序列表，**丢失了键索引**，放大上述 bug 的影响面（错误 `find`、无法按 `remove` 键 O(1) 删除）。

3. **复制粘贴**：两文件结构几乎一致，说明同一错误被**重复引入**，评审与单测未覆盖「多命名空间 + 删除事件」场景。

---

## 修复建议（含具体代码示例）

### 方向 A（推荐）：以 `HashMap<String, T>` 为唯一真源，键 = 上游 `key_name()` 格式

1. 将静态存储改为例如：  
   `Arc<RwLock<HashMap<String, EdgionGatewayConfig>>>`（`GatewayClass` 同理）。

2. **`full_set`**：`store = data.clone()` 或 `clear` 后 `extend(data.iter().map(|(k,v)| (k.clone(), v.clone())))`。

3. **`partial_update`**：  
   - `add` / `update`：使用 **map 的 key** 作为索引：`store.insert(key, config)`（或 `update` 时 `store.insert(key, config)`，与 `add` 统一为 upsert 也可，视产品语义而定）。  
   - `remove`：`for k in remove { store.remove(&k); }`  
   若需兼容历史数据，删除时也可同时尝试 `remove` 裸名键（**不推荐长期保留**，仅作迁移过渡）。

4. **查询 API**：将 `get_*_by_name` 扩展为 `get_*(&str /* key */)` 或提供 `get_*_by_key(namespace, name)`，内部使用 `format!("{}/{}", ns, name)` 与 conf_sync 一致；若保留旧 API，须在文档中标明**仅集群单命名空间场景安全**。

### 方向 B（最小补丁，不推荐作为终点）：在保留 `Vec` 的前提下修正键比较

若短期不能改存储类型，至少应：

```rust
// 删除：用与 remove 集合一致的键（与 kube::ResourceExt 一致）
store.retain(|config| {
    let k = config.key_name(); // 或 format!("{}/{}", ns, name) 与上游一致
    !remove.contains(&k)
});

// 更新：用 map 的 key 匹配，或 key_name()
for (key, config) in update {
    if let Some(existing) = store.iter_mut().find(|c| c.key_name() == key) {
        *existing = config;
    } else {
        tracing::warn!(key = %key, "partial_update update: no existing entry, inserting");
        store.push(config);
    }
}
```

**说明**：方向 B 仍遗留 O(n)、同名跨 ns 的 `Vec` 重复项风险；**方向 A** 才能同时解决性能与语义问题。

### 日志与可观测性

- 对 `update` 未命中：应 **`tracing::warn!`**（或 metrics），并可选 **fallback insert**（与产品语义一致时）。
- `remove` 后对 `remove` 中仍不存在的 key 可打 **debug**，便于发现双重删除或键格式错误。

---

## 测试建议

### 单元测试（每个 handler 或共享测试工具）

1. **`partial_update` 删除**  
   - `full_set` 或 `add` 写入 `ns-a/foo` 与 `ns-b/foo`（键分别为 `ns-a/foo`、`ns-b/foo`）。  
   - `remove = {"ns-a/foo"}`。  
   - 断言：仅剩 `ns-b/foo`；**绝不能**仅按 `foo` 误删两个。

2. **`partial_update` 更新**  
   - 两命名空间同名不同 spec。  
   - `update` 仅带 `ns-b/foo` 的新版本。  
   - 断言：`ns-a` 未变，`ns-b` 已更新。

3. **`update` 无匹配项**  
   - 空 store 或非空 store 下发 `update`（key 存在但 store 无对应项）。  
   - 断言：行为符合约定（插入或告警），**不能静默无操作**。

4. **与 `name_any()` Regression**  
   - 刻意构造 `remove` 含 `namespace/name`，确保 **不再** 使用 `remove.contains(&name_any())` 的错误写法。

### 集成 / 回归

- 在 conf_sync 压缩事件路径下（`cache_data` 调用 `processor.partial_update`）做一次 **端到端**：Watch 删除某 namespace 下 `EdgionGatewayConfig` / `GatewayClass` 后，处理器缓存中对应项消失。

---

## 小结

| 项 | 结论 |
|----|------|
| 根因 | `remove`/`update` 使用 `name_any()` 与上游 `namespace/name` 键不一致；`Vec` 丢失键索引并引入扫描与歧义 |
| 修复核心 | 以 **`HashMap<String, T>`**（或等价结构）按 **与 `key_name()` 相同** 的字符串键存储；`remove`/`update` 均基于该键 |
| 风险若不修 | 删除永不生效、缓存泄漏、跨 ns 同名错更、运维日志误导 |

---

*文档基于仓库当前实现撰写；行号以撰写时 `Edgion` 主分支为准，若后续改动请以实际文件为准。*
