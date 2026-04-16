# Phase 1：Center 端 PluginMetaData 聚合 Store

## 目标

Center 为每个 Controller 维护一份 PluginMetaData 快照，支持增量更新和全量替换。这是 Phase 2（反向 watch）的数据落地层。

## 数据结构

```rust
// 文件：src/core/center/pm_store/mod.rs

/// 单个 Controller 的 PluginMetaData 快照
struct ControllerPmSnapshot {
    /// pm_key ("namespace/name") → PluginMetaData JSON (serde_json::Value)
    items: HashMap<String, serde_json::Value>,
    /// 该 Controller 的最新 sync_version（watch 断线重连时从此版本续接）
    sync_version: u64,
}

/// Center 全局聚合 store
pub struct CenterPmStore {
    /// controller_id → ControllerPmSnapshot
    inner: RwLock<HashMap<String, ControllerPmSnapshot>>,
}
```

## CenterPmStore API

```rust
impl CenterPmStore {
    pub fn new() -> Self;

    /// 全量替换某 controller 的 PM 快照（list 响应）
    pub fn replace_all(&self, controller_id: &str, items: HashMap<String, serde_json::Value>, sync_version: u64);

    /// 增量更新（watch event）
    pub fn apply_event(&self, controller_id: &str, event_type: EventType, pm_key: String, data: serde_json::Value, sync_version: u64);

    /// Controller 离线：保留数据但标记版本，重连时 re-list
    pub fn mark_offline(&self, controller_id: &str);

    /// Controller 被移除：清除数据
    pub fn remove_controller(&self, controller_id: &str);

    /// 获取某 controller 的当前 sync_version（用于 watch 断线重连）
    pub fn get_sync_version(&self, controller_id: &str) -> u64;

    /// 列出所有 PM（聚合视图）：返回 Vec<(pm_key, Vec<controller_id>)>
    pub fn list_aggregated(&self) -> Vec<AggregatedPmEntry>;

    /// 获取某 controller 某 PM 的完整数据
    pub fn get_pm(&self, controller_id: &str, pm_key: &str) -> Option<serde_json::Value>;

    /// 列出某 controller 的所有 PM
    pub fn list_by_controller(&self, controller_id: &str) -> Vec<(String, serde_json::Value)>;
}

pub struct AggregatedPmEntry {
    pub namespace: String,
    pub name: String,
    pub controllers: Vec<String>,  // 哪些 controller 有这个 PM
}
```

## 步骤

### 1.1 创建 `src/core/center/pm_store/mod.rs`

- 实现 `CenterPmStore` 和 `ControllerPmSnapshot`
- `serde_json::Value` 存原始 PM（后续 Phase 3 再解析为结构化数据）
- pm_key 格式：`"namespace/name"`（与 Controller 端 `PluginMetaDataStore::make_key` 一致）

### 1.2 注册到 Center 启动链路

- 在 Center 的 `ApiState`（或等价结构）中添加 `Arc<CenterPmStore>`
- `src/core/center/api/mod.rs` 路由注册时传入

### 1.3 接入 gRPC server

- `src/core/center/fed_sync/server/mod.rs` 的 `FederationGrpcServer` 添加 `pm_store: Arc<CenterPmStore>` 字段
- Controller 注册时初始化空快照
- Controller 断线时调 `mark_offline`

### 1.4 单测

- `replace_all` + `list_aggregated` 基本流
- `apply_event` 增量更新
- 多 controller 聚合
- `remove_controller` 清除

## 依赖

无外部依赖，纯内存数据结构。Phase 2 完成后才有数据流入。
