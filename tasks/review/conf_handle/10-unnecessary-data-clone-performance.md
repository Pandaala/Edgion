# 配置同步路径上对大 `HashMap` 的冗余 `clone` 性能审查

## 问题标题

`ConfHandler::full_set` 采用 `&HashMap<String, T>` 入参，多个实现为对接「需要所有权」的下层存储而**整表深拷贝**；在 Endpoints / EndpointSlice 全量同步场景下会放大内存与 CPU 开销。TLS 与插件路径也存在整表或全量克隆模式，且 TLS 层存在**同名方法**带来的误读与演进风险。

## 严重程度

**中**

- **为何不是「高」**：问题主要在全量同步（`full_set` / relist）路径；常态增量 `partial_update` 中 Endpoints/EndpointSlice 已使用 `update_data_only(add, update, remove)`，**不经过**本次指出的 `replace_data_only(data.clone())` 模式。
- **为何不是「低」**：集群规模较大时，全量表可复制**与缓存中已有的一份数据等量的第二份（及更多中间副本）**，并在控制器重连、周期性 resync 时重复发生，峰值内存与分配器压力明显。

---

## 受影响场景与数据量估算

| 场景 | 触发条件 | 典型数据量（量级） | 备注 |
|------|----------|-------------------|------|
| Endpoints 全量 | `EndpointHandler::full_set` | 数百～数千 Service 对应的 `Endpoints` | 每个 `Endpoints` 含多 subset、多地址，体积随 Pod 数增长 |
| EndpointSlice 全量 | `EpSliceHandler::full_set` | Slice 数常高于 Endpoints 对象数 | 单 Slice 可为数 KB～数十 KB（含 `endpoints`、`ports`、`metadata`） |
| HTTP 插件全量 | `PluginStore::full_set` | CR 数量通常远小于 Endpoint | 单条 `EdgionPlugins` 视 spec 复杂度变化大 |
| TLS 全量 | `Arc<TlsStore>` 的 `ConfHandler::full_set` | 证书/网关绑定条数通常较小 | 但每条含证书链、密钥等，**单条体积大**，整表克隆仍不可忽视 |
| EdgionGatewayConfig | `EdgionGatewayConfigHandler::full_set` | 一般极少（个位数～十位数） | 按条 `clone` 写入 `Vec`，与「整表一次性克隆」性质不同 |

**粗算示例（用于评估数量级，非 SLA）**：若 3000 个 EndpointSlice 平均 8KB，仅值侧约 24MB；再叠加 `HashMap` 重新分配与键克隆，瞬时分配可更高。全量同步若同时触发多类资源的 handler，压力叠加。

---

## 根因：`ConfHandler` 与调用方所有权模型

`ConfHandler` 定义（`src/core/common/conf_sync/traits.rs`）：

```rust
fn full_set(&self, data: &HashMap<String, T>);
```

- 实现方若要把数据写入内部 `RwLock<HashMap<...>>` 且 API 要求 `HashMap` 所有权，则必须在实现内 **`clone()` 整表**。
- **重要约束**：`CacheData::reset` 在构建 `self.data` 后调用 `handler.processor.full_set(&self.data)`，且**之后仍保留** `self.data` 供 `get`/`insert` 等使用（见 `src/core/gateway/conf_sync/cache_client/cache_data.rs`）。因此「直接把 `HashMap` move 进 handler」会掏空缓存，除非同步调整缓存与数据源架构。

---

## 各文件 `clone` 开销分析（已与源码核对）

### 1. `endpoint/conf_handler_impl.rs`（约第 124 行）

```rust
let all_services = roundrobin_store.replace_data_only(data.clone());
```

- **`replace_data_only` 签名**：`pub fn replace_data_only(&self, data: HashMap<String, Endpoints>) -> HashSet<String>`（`endpoint_store.rs`）。
- **开销**：对传入的 `&HashMap` 做**完整深拷贝**（所有 key、`Endpoints` 值），再 `*self.ep_data.write().unwrap() = data` 移入数据层。
- **与缓存关系**：网关缓存 `CacheData` 中已持有一份相同资源的 `HashMap`；此处再复制一份进入 RoundRobin 的 `ep_data`，形成**至少双份**常驻（缓存 + 数据层）。

### 2. `endpoint_slice/conf_handler_impl.rs`（约第 148 行）

与 Endpoints 路径对称：

```rust
let all_services = roundrobin_store.replace_data_only(data.clone());
```

- **开销**：整表克隆 `HashMap<String, EndpointSlice>`；Slice 资源通常比单个 Endpoints 更碎、总字节数往往更大。

### 3. `plugins/http/conf_handler_impl.rs`（约第 35–39 行）

```rust
let mut prepared_data = data.clone();
for plugin in prepared_data.values_mut() {
    plugin.preparse();
}
self.replace_all(prepared_data);
```

- **开销**：为对值做可变 `preparse()`，先克隆整张表；随后 `replace_all` 再按 store 实现可能继续分配或移动。
- **性质**：非「仅传递所有权」的克隆，而是**刻意**在副本上变异；仍可通过 API 设计（见下文建议）减少一次全表拷贝。

### 4. `tls/store/conf_handler.rs`（约第 10、26 行）

```rust
// Arc<TlsStore>
fn full_set(&self, data: &HashMap<String, EdgionTls>) {
    (**self).full_set(data.clone())
}

// TlsStore 的 ConfHandler 实现
fn full_set(&self, data: &HashMap<String, EdgionTls>) {
    self.full_set(data.clone())
}
```

- **实际主路径**：`create_tls_handler()` 返回 `Box<dyn ConfHandler<...>>` 包装 `Arc<TlsStore>`（`get_global_tls_store()`）。动态分发进入 **`Arc<TlsStore>` 的 `full_set`**，其中 `(**self).full_set(data.clone())` 因参数类型为 **`HashMap`**，解析为 **`TlsStore` 固有的 `full_set(HashMap<String, EdgionTls>)`**，而非再次进入 `ConfHandler::full_set`。
- **结论**：**当前生产调用链上，TLS 全量同步对同一份输入通常为「一次整表 `HashMap` 克隆」**，不会在单次调用中连续两次整表克隆。
- **风险**：`ConfHandler::full_set` 与 `TlsStore::full_set(HashMap)` **同名**，维护时极易误判为「双重克隆」或错误改为 trait 递归调用；与既有文档 `07-tls-store-method-name-ambiguity.md` 主题一致，建议重构命名或委托方式。

### 5. `config/edgion_gateway/conf_handler_impl.rs`（约第 52–58 行）

```rust
for (key, config) in data {
    store.push(config.clone());
}
```

- **开销**：按条目 `clone` 到 `Vec<EdgionGatewayConfig>`；资源条数通常很少，**不属于本次「整表 HashMap 克隆」主矛盾**。
- **备注**：若未来网关实例 CR 数量剧增，可再评估是否改为 `extend` 拥有所有权的迭代器或 `Arc` 化配置。

---

## `replace_data_only` 与短期优化方向

当前实现（以 Endpoint 为例）在替换数据层时直接接收并拥有 `HashMap`：

```rust
pub fn replace_data_only(&self, data: HashMap<String, Endpoints>) -> HashSet<String> {
    let keys: HashSet<String> = data.keys().cloned().collect();
    *self.ep_data.write().unwrap() = data;
    keys
}
```

**短期建议（与用户诉求一致，稍作细化）**：

1. **改为接收 `HashMap` 的可变借用或内部 swap**  
   - 例如 `replace_data_only(&self, new_data: HashMap<...>)` 改为在持锁情况下 `std::mem::replace` / `swap`，避免 handler 侧先 `clone` 再 move（handler 仍须解决「只有 `&HashMap`」的问题，见下条）。
2. **或改为 `fn replace_data_only(&self, data: &HashMap<String, Endpoints>)`**  
   - 在**单处**持写锁时从 `&HashMap` 构建新 map 或逐条插入：若仍要完整复制值，**总复制量未必减少**，但可把「克隆策略」收拢到 store 内，便于后续改为浅拷贝/`Arc`。
3. **更优短期组合**：`ConfHandler` 调用路径上若能在**不破坏 `CacheData` 语义**的前提下，对「仅用于 full_set 下沉存储」的分支 **move 或 `take` 出缓存 map**（需产品化评估），则可彻底去掉本次多余的一份克隆；否则应在 store 层用 **`clone_from` / 容量复用** 等降低分配峰值（视 `T` 是否实现 `clone_from` 而定）。

---

## Trait 设计改进方案（中期）

### 方案 A：`full_set` 接收所有权

```rust
fn full_set(&self, data: HashMap<String, T>);
```

- **优点**：调用方可在**不再需要缓存副本**时 move，避免实现侧强制 `clone`。
- **与现状冲突**：`CacheData::reset` 在 `full_set` 后仍依赖 `self.data`。若要采用本方案，需其一：  
  - 缓存改为只存元数据/指针，业务数据仅以各 handler/store 为准；或  
  - `reset` 在 `full_set` 之后从某处回填缓存（可能再次引入拷贝或共享指针）。

### 方案 B：拆分「只读全量通知」与「交付所有权」

- 例如增加 `full_set_owned(self: &mut CacheData, ...)` 或在 sync 管道中区分 **relist 专用路径**，使数据只流经 store 一次。

### 方案 C：`full_set` 接受 `impl IntoIterator<Item = (String, T)>` 或一次性格式

- 减少中间 `HashMap` 形态，由最上游 `reset` 直接 `collect` 进 store（需梳理锁顺序与错误处理）。

**推荐优先级**：在明确 `CacheData` 与各 `*Store` 的**单一数据源**策略后，再选 A/B；否则优先做 **store 层 API 与克隆位置收敛**（短期）。

---

## 长期建议：`Arc<T>` 与共享不可变快照

- 将 `HashMap<String, Arc<T>>` 作为跨缓存与数据层的**共享句柄**，`full_set` 时更新 `Arc` 指向新构建的快照，旧快照在无引用后释放。
- 与 Pingora / 网关内已有 `ArcSwap` 等模式对齐时，可显著降低大表深拷贝频率。
- **代价**：类型与序列化边界需统一；`partial_update` 需明确是替换 `Arc` 还是 copy-on-write。

---

## 短期 / 中期 / 长期修复建议汇总

| 阶段 | 建议 | 预期收益 |
|------|------|----------|
| **短期** | `replace_data_only` / 同类 API 接受 `&HashMap` 或内部 swap；克隆逻辑内聚到 store；评估 `TlsStore` 与 `ConfHandler` 同名 `full_set` 的重命名 | 降低重复实现与误改风险；为后续减少拷贝铺路 |
| **短期** | `PluginStore::full_set`：提供 `replace_all` 的变体，接受 `&HashMap` + 在插入路径调用 `preparse`，或 `preparse` 改为可作用于 `&mut` 且由 `replace_all` 内部借用 | 去掉「先整表 clone 再 preparse」 |
| **中期** | 梳理 `CacheData` 与 handler 数据所有权，将 `ConfHandler::full_set` 改为 `HashMap` 所有权或 `&mut HashMap` 交换策略 | 在架构允许时消除「缓存 + store」双份全量深拷贝 |
| **长期** | 大资源统一 `Arc` 化或版本化快照；全量路径与增量路径共用同一套廉价更新语义 | 大规模集群下内存与 GC/分配器压力显著下降 |

---

## 验证与回归关注点

- 全量同步后 **健康检查、selector 缓存、陈旧 Service 清理** 等行为与现网一致（Endpoints / EndpointSlice handler 依赖 `replace_data_only` 返回的 key 集合）。
- TLS 证书校验失败路径、matcher 重建顺序（`tls_store.rs` 中写锁内逻辑）在 API 变更后不变。
- 插件 `preparse` 副作用（若存在）在「不再预先 clone」的实现中仍只作用于最终入库对象。

---

## 参考代码位置

- `ConfHandler`：`src/core/common/conf_sync/traits.rs`
- 缓存调用：`src/core/gateway/conf_sync/cache_client/cache_data.rs`（`reset` 内 `full_set(&self.data)`）
- Endpoint 数据层：`src/core/gateway/backends/discovery/endpoint/endpoint_store.rs`（`replace_data_only`）
- EndpointSlice 数据层：`src/core/gateway/backends/discovery/endpoint_slice/ep_slice_store.rs`（`replace_data_only`）
- TLS 固有 `full_set`：`src/core/gateway/tls/store/tls_store.rs`

---

*文档基于仓库当前源码核对编写；行号随分支可能略有漂移。*
