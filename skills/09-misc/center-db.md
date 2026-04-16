---
name: center-db
description: Center 联邦持久化：SQLite controllers 表、CenterDb 结构体、database 配置。新的 PluginMetaData 联邦数据流通过反向 Watch 同步（CenterWatchCache → CenterMetaDataStore），无 SQLite 缓存层。
---

# Center 联邦持久化（SQLite）

## 背景与核心决策

Center 使用 SQLite 持久化 **controller 注册信息**（上下线状态、标签等），保证 Center 重启后不丢失已注册的 controller 记录。

**为什么选 SQLite bundled 而非运行时依赖：**
- `rusqlite = { features = ["bundled"] }` 在编译期从 C 源码编译 SQLite 并静态链接
- 运行时不需要 `libsqlite3.so`，部署到 ubuntu:24.04 的 `Dockerfile.runtime` 无需任何改动
- 构建端 `Dockerfile.builder`（rust:bookworm，含 gcc）也无需改动——C 源码编译已被 cargo 处理
- 对比"系统动态库"方案：bundled 的好处是零运行时依赖，代价是首次编译慢约 30 秒（C 代码编译）

**并发访问模型：**
- 单个 `Arc<Mutex<Connection>>`（`parking_lot::Mutex`）
- 所有 DB 操作包在 `spawn_blocking` 中执行，不阻塞异步运行时
- 写操作串行化（Mutex），读操作也串行（同一 Connection）——Center 写压力低，这是合理简化

## DB Schema

```sql
CREATE TABLE controllers (
    controller_id TEXT PRIMARY KEY,
    cluster       TEXT NOT NULL DEFAULT '',
    env           TEXT NOT NULL DEFAULT '[]',   -- JSON 数组
    tag           TEXT NOT NULL DEFAULT '[]',   -- JSON 数组
    online        INTEGER NOT NULL DEFAULT 0,
    last_seen_at  INTEGER NOT NULL,
    created_at    INTEGER NOT NULL
);
```

设计要点：
- `controllers` 表持久化 controller 的注册元信息，Center 重启后可恢复已知 controller 列表
- `online` 字段在 controller 连接/断开 federation gRPC 流时更新

> **已移除的表**：`cluster_plugin_metadata_cache` 和 `service_plugin_metadata_cache`（曾用于缓存 PluginMetaData 轮询结果）已随 PM 反向 Watch 重构一并删除，不再存在于 schema 中。

## 配置（edgion-center.yaml）

```yaml
database:
  enabled: true
  sqlite_path: data/center.db  # 相对路径，以 work_dir 为基准
  sync_interval_secs: 60       # 已弃用，保留用于向后兼容，实际不再使用
```

- `enabled: false` 时所有依赖 DB 的 Admin API（如 `GET /admin/controllers`）返回 503
- `sync_interval_secs`：**向后兼容字段**，旧版本用于控制 HTTP 轮询间隔；新架构通过反向 Watch 实时推送，此字段保留但不生效

## PluginMetaData 数据流：反向 Watch 架构

PluginMetaData（集群/服务维度的路由元数据）不再通过 SQLite 缓存，而是走 **federation gRPC 双向流反向 Watch**：

```
Controller（数据源）
    │  FedWatchEventResponse（推送）
    ▼
CenterSyncClient（per-controller gRPC stream 客户端）
    │
    ▼
CenterWatchCache<T>（per-controller 内存缓存，强类型）
    │
    ▼
CenterMetaDataStore（全局聚合视图，跨 controller 合并）
    │
    ▼
API Handler（直接读内存，无 SQLite）
```

关键特性：
- **实时推送**：Controller 资源变更立即通过 FedWatchEventResponse 推送到 Center，无轮询延迟
- **无 SQLite PM 缓存**：PluginMetaData 完全存活于内存；Controller 离线时其数据从聚合视图中移除（不保留历史快照）
- **强类型**：`CenterWatchCache<T>` 泛型设计，`T` 目前包含 ClusterRegionRoute PM 和 ServiceRegionRoute PM

详细设计见项目设计规范（如有 ADR 文档，从 `docs/` 或 `tasks/` 中查阅）。

## Admin API

所有端点在 `database.enabled = false` 时返回 503。

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/center/admin/controllers` | 列出 DB 中持久化的 controller 记录（含离线记录） |
| `DELETE` | `/api/v1/center/admin/controllers/{id}` | 删除指定 controller 记录及其所有关联数据 |
| `GET` | `/api/v1/center/cluster-region-routes` | 实时聚合的集群维度 RegionRoute PM（来自内存，非 SQLite） |
| `GET` | `/api/v1/center/service-region-routes` | 实时聚合的服务维度 RegionRoute PM（来自内存，非 SQLite） |
| `GET` | `/api/v1/center/consistency` | 一致性检查端点 |

**典型运维场景：**
- Controller 永久下线 → `DELETE /admin/controllers/{id}` 清除其持久化记录
- 查看当前实时 PM 数据 → `GET /cluster-region-routes` 或 `/service-region-routes`（内存数据，实时）

## Docker 构建注意事项

`rusqlite bundled` 在 **编译期** 完成所有工作：
- `Dockerfile.builder`（rust:bookworm + gcc）：无需额外包，gcc 已包含
- `Dockerfile.runtime`（ubuntu:24.04 + libssl3）：无需安装 `libsqlite3`，SQLite 已静态链接进二进制

唯一的影响是首次全量编译多约 30 秒（C 代码编译），之后 cargo 缓存层命中不会重复编译。

## 源码位置

| 模块 | 路径 |
|------|------|
| DB 初始化 & 连接管理 | `src/core/center/db/mod.rs` |
| Schema 建表 SQL | `src/core/center/db/schema.rs`（或 `mod.rs` 内联） |
| CenterDb（controller upsert/delete） | `src/core/center/db/mod.rs` |
| Admin API handlers | `src/core/center/api/admin_handlers.rs` |
| Center 配置结构 | `src/core/center/config.rs`（`DatabaseConfig` 字段） |
| **反向 Watch 缓存层** | `src/core/center/watch_cache/`（CenterWatchCache、CenterWatchCacheRegistry、CenterSyncClient、CenterConfHandler） |
| **PM 聚合视图** | `src/core/center/metadata_store/`（CenterMetaDataStore） |
| **共享解析工具** | `src/core/common/metadata_conf_handler.rs`（parse_region_route() 等） |
