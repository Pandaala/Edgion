# Phase 5：AA 操作 + 多集群一致性检测

## 目标

### Center 端

1. **Failover 扇出**：Center 收到 failover 请求后，通过 gRPC proxy 转发到所有相关 Controller（已有逻辑，需适配新 API 路径）
2. **多集群配置一致性检测**：比较同一个 PM 在不同 Controller 上的配置，检测不一致

### Controller 端

1. **本地 Failover**：修改本地 PluginMetaData 的 region failoverTo（已有 `patch_plugin_metadata_failover`，需适配新 API 路径）

## Center AA 操作

### Failover API

```
POST /api/v1/center/cluster-region-routes/failover
POST /api/v1/center/service-region-routes/failover
```

请求体不变：
```json
{
  "namespace": "ns",
  "name": "pm-name",
  "regionName": "east",
  "failoverTo": "west"
}
```

实现：沿用现有 `fan_out_failover()` 逻辑，通过 gRPC proxy 转发到每个 Controller 的对应 API。

### 一致性检测 API

```
GET /api/v1/center/cluster-region-routes/consistency
GET /api/v1/center/service-region-routes/consistency
```

从 `CenterMetaDataStore` 读取数据，比较同一 key 下各 controller 的配置：

```rust
pub struct ConsistencyReport {
    pub key: String,               // PM key
    pub consistent: bool,
    pub conflicts: Vec<ConflictDetail>,
}

pub struct ConflictDetail {
    pub field: String,             // "regions[east].hashRange" / "regions[east].failoverTo" 等
    pub values: HashMap<String, String>,  // controller_id → 该 controller 的值
}
```

比较的字段：
- ClusterRegionRoute：`regions[].hashRange`、`regions[].backendEndpoint`、`regions[].failoverTo`、`routeRules`
- ServiceRegionRoute：`regions[].failoverTo`

## Controller AA 操作

### Failover API

```
POST /api/v1/cluster-region-routes/failover
POST /api/v1/service-region-routes/failover
```

复用现有 `patch_plugin_metadata_failover()` 逻辑（`region_route_handlers.rs`），只是 API 路径改变。

## 步骤

### 5.1 Center Failover API（适配新路径）

- 新建或修改 handler，路径从 `/center/cluster-region-pms/failover` 改为 `/center/cluster-region-routes/failover`
- 内部逻辑复用 `fan_out_failover()`，转发到 Controller 的新路径

### 5.2 Controller Failover API（适配新路径）

- 注册新路由 `/api/v1/cluster-region-routes/failover` 和 `/api/v1/service-region-routes/failover`
- handler 复用 `patch_plugin_metadata_failover()`

### 5.3 Center 一致性检测

- `src/core/center/api/consistency_handlers.rs`（或在 metadata handler 中）
- 从 `CenterMetaDataStore.cluster_region_routes` 读取
- 对每个 key，比较各 controller 的 PM 配置
- 返回 `Vec<ConsistencyReport>`

### 5.4 前端一致性展示

- ClusterRegionRoute 列表页增加一致性状态列（一致/冲突）
- 冲突项展开后显示具体差异
- Center 模式独有功能

### 5.5 前端 Failover UI

- 对接新 API 路径
- 复用现有 FailoverPanel 组件（如需修改）
