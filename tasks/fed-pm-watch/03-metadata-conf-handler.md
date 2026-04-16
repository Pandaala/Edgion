# Phase 3：MetadataConfHandler 解析层

## 目标

将 `CenterPmStore` 中的原始 `PluginMetaData` 和 Controller 本地的 `PluginMetaData` 解析为结构化的 RegionRoute 数据。Controller 和 Center 各自有 store，共享解析逻辑，区别在于 value 类型：

| | key | value |
|---|---|---|
| Controller | pm_key | 单个 PM 值 |
| Center | pm_key | `HashMap<controller_id, PM 值>` |

## 数据结构

### 共享类型（common 或 types 模块）

```rust
/// ClusterRegionRoute 的 key：namespace/name
/// 直接用 pm_key (String) 作为 key

/// ServiceRegionRoute 的 key：ref_cluster_ns/ref_cluster_name/namespace/name
/// 因为同一个 ServiceRegionRoute PM 绑定到某个 ClusterRegionRoute，key 需要包含引用关系
pub struct ServiceRegionRouteKey {
    pub cluster_ref_namespace: String,
    pub cluster_ref_name: String,
    pub namespace: String,
    pub name: String,
}

impl ServiceRegionRouteKey {
    pub fn to_string_key(&self) -> String {
        format!("{}/{}/{}/{}", self.cluster_ref_namespace, self.cluster_ref_name, self.namespace, self.name)
    }
}
```

### Controller 端：ControllerMetaDataStore

```rust
// 文件：src/core/controller/conf_mgr/sync_runtime/resource_processor/metadata_store.rs
// 或 src/core/controller/metadata_store/mod.rs

pub struct ControllerMetaDataStore {
    /// ClusterRegionRoute: key = "namespace/name" → PM value
    pub cluster_region_routes: RwLock<HashMap<String, serde_json::Value>>,
    /// ServiceRegionRoute: key = "ref_ns/ref_name/ns/name" → PM value  
    pub service_region_routes: RwLock<HashMap<String, serde_json::Value>>,
}
```

### Center 端：CenterMetaDataStore

```rust
// 文件：src/core/center/metadata_store/mod.rs

pub struct CenterMetaDataStore {
    /// ClusterRegionRoute: key = "namespace/name" → HashMap<controller_id, PM value>
    pub cluster_region_routes: RwLock<HashMap<String, HashMap<String, serde_json::Value>>>,
    /// ServiceRegionRoute: key = "ref_ns/ref_name/ns/name" → HashMap<controller_id, PM value>
    pub service_region_routes: RwLock<HashMap<String, HashMap<String, serde_json::Value>>>,
}
```

### 共享解析逻辑：MetadataConfHandler

```rust
// 文件：src/core/common/metadata_conf_handler.rs

/// 从原始 PluginMetaData JSON 中提取类型和 key
pub enum ParsedPmType {
    ClusterRegionRoute {
        key: String,             // "namespace/name"
        data: serde_json::Value, // 完整 PM
    },
    ServiceRegionRoute {
        key: String,             // "ref_ns/ref_name/ns/name"
        data: serde_json::Value,
    },
    Other,  // 非 RegionRoute 类型，忽略
}

/// 解析单个 PluginMetaData JSON
pub fn parse_plugin_metadata(pm: &serde_json::Value) -> ParsedPmType {
    // 1. 提取 metadata.namespace, metadata.name
    // 2. 判断 spec.metadata.type:
    //    - "ClusterRegionRoute" → key = "ns/name"
    //    - "ServiceRegionRoute" → 从 spec.metadata.config.clusterRef 提取 ref_ns/ref_name
    //                             key = "ref_ns/ref_name/ns/name"
    //    - 其他 → Other
}
```

## 步骤

### 3.1 实现共享解析逻辑

- `src/core/common/metadata_conf_handler.rs`
- `parse_plugin_metadata()` 函数
- 单测：ClusterRegionRoute PM → 正确 key，ServiceRegionRoute PM → 含 clusterRef 的 key，其他类型 → Other

### 3.2 实现 ControllerMetaDataStore

- `src/core/controller/metadata_store/mod.rs`（或合适位置）
- 在 `ControllerRuntime` 中添加 `metadata_store: ControllerMetaDataStore`
- 在 `PluginMetadataHandler.parse()` 和 `on_delete()` 中调用：
  ```rust
  let parsed = parse_plugin_metadata(&pm_json);
  match parsed {
      ParsedPmType::ClusterRegionRoute { key, data } => {
          ctx.runtime().metadata_store.cluster_region_routes.write().insert(key, data);
      }
      ParsedPmType::ServiceRegionRoute { key, data } => {
          ctx.runtime().metadata_store.service_region_routes.write().insert(key, data);
      }
      ParsedPmType::Other => {}
  }
  ```
- `reset()` 时清空
- 注意：现有的 `RegionRouteClusterTable` / `RegionRouteServiceTable` **暂时保留**（Gateway 还在用），后续 Phase 6 评估是否合并或删除

### 3.3 实现 CenterMetaDataStore

- `src/core/center/metadata_store/mod.rs`
- 接口：

```rust
impl CenterMetaDataStore {
    /// 当 CenterPmStore 收到 list/watch 事件后调用
    /// 解析所有 PM，按类型分发到 cluster_region_routes / service_region_routes
    pub fn apply_full_set(&self, controller_id: &str, pms: &HashMap<String, serde_json::Value>);

    /// 增量更新单个 PM
    pub fn apply_pm_event(&self, controller_id: &str, event_type: EventType, pm_key: &str, data: &serde_json::Value);

    /// 删除某 PM
    pub fn remove_pm(&self, controller_id: &str, pm_key: &str, data: &serde_json::Value);

    /// 清除某 controller 的全部数据
    pub fn remove_controller(&self, controller_id: &str);

    /// 列出 ClusterRegionRoute（前端 API 用）
    pub fn list_cluster_region_routes(&self) -> Vec<CenterClusterRegionRouteEntry>;

    /// 列出 ServiceRegionRoute（前端 API 用）
    pub fn list_service_region_routes(&self) -> Vec<CenterServiceRegionRouteEntry>;
}

/// 前端 API 响应类型
pub struct CenterClusterRegionRouteEntry {
    pub namespace: String,
    pub name: String,
    pub controllers: HashMap<String, serde_json::Value>,  // controller_id → PM 配置
}

pub struct CenterServiceRegionRouteEntry {
    pub cluster_ref_namespace: String,
    pub cluster_ref_name: String,
    pub namespace: String,
    pub name: String,
    pub controllers: HashMap<String, serde_json::Value>,
}
```

### 3.4 接入 CenterPmStore → CenterMetaDataStore

在 Phase 2 的 `replace_all` 和 `apply_event` 调用后，联动更新 `CenterMetaDataStore`：

```rust
// Center gRPC server 收到 WatchListResponse 后：
pm_store.replace_all(controller_id, items, sync_version);
metadata_store.apply_full_set(controller_id, &items);

// 收到 WatchEventResponse 后：
pm_store.apply_event(controller_id, event_type, pm_key, data, sync_version);
metadata_store.apply_pm_event(controller_id, event_type, &pm_key, &data);
```

### 3.5 新增 Center API

替换现有的 fan-out 聚合 API（暂时并存，Phase 6 删老的）：

```
GET /api/v1/center/cluster-region-routes   → metadata_store.list_cluster_region_routes()
GET /api/v1/center/service-region-routes   → metadata_store.list_service_region_routes()
```

### 3.6 新增 Controller API

与 Center API 对称：

```
GET /api/v1/cluster-region-routes   → metadata_store.list_cluster_region_routes()
GET /api/v1/service-region-routes   → metadata_store.list_service_region_routes()
```

注意：Controller 端返回的是单值，Center 端返回的是 `HashMap<controller_id, value>`。可以用同一个响应结构，Controller 只有一个 key。或者分开定义，前端根据 mode 选择。

### 3.7 单测

- 解析 ClusterRegionRoute PM → 正确 key
- 解析 ServiceRegionRoute PM（含 clusterRef）→ 正确复合 key
- CenterMetaDataStore 多 controller 聚合
- apply_full_set 后 list 正确
- 增量 event 更新正确
- remove_controller 清除干净
