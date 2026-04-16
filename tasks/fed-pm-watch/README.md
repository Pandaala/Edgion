# PluginMetaData 联邦反向 Watch 同步

## 为什么做

当前 Center 获取 RegionRoute 数据的方式是：前端请求 → Center API → 通过 gRPC proxy 实时 fan-out 到所有在线 Controller → 聚合返回。每次请求都是全量 fan-out，没有本地缓存。另外 `db/sync.rs` 定期轮询也是通过 proxy 拉 Controller HTTP API，存入 SQLite cache 表。

问题：
- 每次前端查询都要 fan-out 全部 Controller，延迟高
- SQLite cache 是冗余中间层，数据可能过时
- Center 没有结构化的 PluginMetaData 内存视图，无法做一致性检测等高级功能

## 做什么

Center 通过已有的 gRPC 双向流（`FederationSync.Sync`），**反向 list/watch** Controller 的 `PluginMetaData`。复用 Controller 已有的 `ConfigSyncServer` → `ServerCache<PluginMetaData>` 完整 sync_version 机制，零新增 Controller 端 store。

Center 收到 PluginMetaData 后，自行解析为 ClusterRegionRoute / ServiceRegionRoute，存入结构化的聚合 store，供 API 和前端消费。

## 架构变化

```
之前：
  前端 → Center API → gRPC proxy fan-out → 每个 Controller HTTP API → 聚合返回

之后：
  Controller                         Center
  ServerCache<PluginMetaData>        CenterMetaDataStore
  （已有，Gateway 也用）               （新增，聚合所有 Controller）
       │                                  │
       │── gRPC 双向流 watch ──────────▶  │ 实时更新
       │                                  │
                                     Center API ← 前端（直接读内存）
```

## 文件索引

| 文件 | 内容 |
|------|------|
| [01-center-pm-store.md](01-center-pm-store.md) | Phase 1：Center 端 PluginMetaData 聚合 store |
| [02-reverse-watch.md](02-reverse-watch.md) | Phase 2：gRPC 反向 list/watch 实现 |
| [03-metadata-conf-handler.md](03-metadata-conf-handler.md) | Phase 3：MetadataConfHandler 解析层（Controller + Center） |
| [04-aa-operations.md](04-aa-operations.md) | Phase 4：Center/Controller AA 操作 + 一致性检测 |
| [05-cleanup.md](05-cleanup.md) | Phase 5：清理旧 API 和 store |
| [06-frontend-rebuild.md](06-frontend-rebuild.md) | Phase 6：前端 RegionRoute 页面重建 |

## 关键现有代码索引

| 模块 | 路径 | 作用 |
|------|------|------|
| proto 定义 | `src/core/common/fed_sync/proto/fed_sync.proto` | gRPC 消息定义 |
| fed_client | `src/core/controller/fed_sync/fed_client/mod.rs` | Controller 连 Center 的 gRPC 客户端 |
| ConfigSyncServer | `src/core/controller/conf_sync/conf_server/config_sync_server.rs` | list/watch 注册中心 |
| ServerCache | `src/core/controller/conf_sync/cache_server/cache.rs` | 带 EventStore 的 watch 缓存 |
| EventStore | `src/core/controller/conf_sync/cache_server/store.rs` | 环形队列 + sync_version |
| WatchObj trait | `src/core/controller/conf_sync/conf_server/traits.rs` | list_json/watch_json 接口 |
| Center gRPC server | `src/core/center/fed_sync/server/mod.rs` | Center 端 gRPC 流处理 |
| Center aggregator | `src/core/center/aggregator/mod.rs` | 现有 ResourceKey 聚合（将被扩展） |
| Center proxy | `src/core/center/proxy/mod.rs` | gRPC 流 HTTP 代理 |
| Controller region_route_handlers | `src/core/controller/api/region_route_handlers.rs` | 现有 Controller RegionRoute API |
| Center region_pm_handlers | `src/core/center/api/region_plugin_metadata_handlers.rs` | 现有 Center 聚合 API |
| PluginMetadataHandler | `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugin_metadata.rs` | PM 处理 + RegionRoute table 更新 |
| RegionRoute tables | `src/core/controller/conf_mgr/sync_runtime/resource_processor/region_route_table/` | 现有 Cluster/Service table |
| ControllerRuntime | `src/core/controller/conf_mgr/runtime.rs` | Controller 所有共享状态 |
