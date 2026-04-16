# Phase 6：清理旧 API 和 Store

## 目标

删除被新架构替代的旧代码，确保没有残留引用。

## 待删除的 API

### Controller 端（GET 类，被 watch 同步替代）

| API | 文件 | 替代方式 |
|-----|------|---------|
| `GET /api/v1/cluster-region-pms` | `region_route_handlers.rs` | Center 通过 watch 获取 |
| `GET /api/v1/service-region-pms` | `region_route_handlers.rs` | Center 通过 watch 获取 |

注意：这两个 GET API 除了 Center 调用外，前端 ControllerProxy 模式也在用。需确认前端改为使用新的 `/api/v1/cluster-region-routes` 后再删。

### Controller 端（POST 类，保留但改路径）

| 旧 API | 新 API | 说明 |
|--------|--------|------|
| `POST /api/v1/cluster-region-pms/failover` | `POST /api/v1/cluster-region-routes/failover` | Phase 5 已添加新路径 |
| `POST /api/v1/service-region-pms/failover` | `POST /api/v1/service-region-routes/failover` | Phase 5 已添加新路径 |

旧 POST 路径在新路径稳定后删除。

### Center 端（被本地 store 替代）

| API | 文件 | 替代方式 |
|-----|------|---------|
| `GET /api/v1/center/cluster-region-pms` | `region_plugin_metadata_handlers.rs` | 新 API 读 CenterMetaDataStore |
| `GET /api/v1/center/service-region-pms` | `region_plugin_metadata_handlers.rs` | 新 API 读 CenterMetaDataStore |
| `POST /api/v1/center/cluster-region-pms/failover` | `region_plugin_metadata_handlers.rs` | 新路径 Phase 5 |
| `POST /api/v1/center/service-region-pms/failover` | `region_plugin_metadata_handlers.rs` | 新路径 Phase 5 |

### Center 端（sync 相关，被 watch 替代）

| API | 文件 | 说明 |
|-----|------|------|
| `POST /api/v1/center/admin/sync` | `admin handlers` | 手动触发同步，watch 后不需要 |
| `DELETE /api/v1/center/admin/cache` | `admin handlers` | 清空 SQLite cache，watch 后不需要 |

## 待删除的 Store / 模块

### Controller 端

| 模块 | 文件 | 替代 |
|------|------|------|
| `RegionRouteClusterTable` | `region_route_table/cluster_table.rs` | `ControllerMetaDataStore.cluster_region_routes` |
| `RegionRouteServiceTable` | `region_route_table/service_table.rs` | `ControllerMetaDataStore.service_region_routes` |
| `region_route_table/mod.rs` | | 整个模块删除 |

需同时从 `ControllerRuntime` 和 `reset()` 中移除引用。

注意：确认 Gateway 插件运行时不直接读这两个 table 后再删。如果 Gateway RegionRoute 插件通过 conf_sync watch 消费 PluginMetaData（而非直接读这些 table），则可安全删除。

### Center 端

| 模块 | 文件 | 说明 |
|------|------|------|
| `db/sync.rs` | `src/core/center/db/sync.rs` | 定期轮询同步任务 |
| SQLite `cluster_plugin_metadata_cache` 表 | `db/mod.rs` schema | |
| SQLite `service_plugin_metadata_cache` 表 | `db/mod.rs` schema | |
| `region_plugin_metadata_handlers.rs` | `src/core/center/api/` | 整个文件删除 |
| `forward_to_all_online` helper | `region_route_handlers.rs` 中的 pub 函数 | 仅被旧 handler 引用 |

## 步骤

### 6.1 确认无外部引用

- `cargo check` 在删除前通过
- grep 所有删除目标的引用点

### 6.2 删除 Controller 端

- 删除 `region_route_table/` 模块
- 从 `ControllerRuntime` 移除字段和 `reset()` 调用
- 从 `PluginMetadataHandler` 移除对旧 table 的写入（Phase 3 已切换到新 store）
- 删除 `region_route_handlers.rs` 中的旧 GET handler
- 删除旧 POST handler（如已迁移到新路径）
- 更新 `api/mod.rs` 路由注册

### 6.3 删除 Center 端

- 删除 `region_plugin_metadata_handlers.rs`
- 删除 `db/sync.rs` 和相关启动调用
- 从 SQLite schema 中移除两张 cache 表（或 `DROP TABLE IF EXISTS`）
- 更新 `api/mod.rs` 路由注册
- 删除 admin sync/cache 相关 handler

### 6.4 清理前端

- 删除 `src/api/center.ts` 中的旧 RegionRoute 方法（如未在 Phase 4 清理）
- 确认无残留引用

### 6.5 验证

- `cargo check --all-targets`
- `cargo clippy --all-targets`
- 前端 `npm run build`
- 集成测试
