---
name: center-db
description: Center 联邦持久化：SQLite DB 设计、后台同步任务、离线回退逻辑、Admin API。有关 Center 如何在 controller 离线后保留 RegionRoute 数据时阅读。
---

# Center 联邦持久化（SQLite）

## 背景与核心决策

Center 需要在 controller 离线后仍能返回 RegionRoute 数据，因此引入 SQLite 作为持久化层。

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

CREATE TABLE region_route_cache (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    controller_id   TEXT NOT NULL,
    service_group   TEXT NOT NULL,
    cluster         TEXT NOT NULL,
    namespace       TEXT NOT NULL,
    kind            TEXT NOT NULL DEFAULT '',
    resource_name   TEXT NOT NULL,
    entries_json    TEXT NOT NULL DEFAULT '[]',  -- JSON 数组，原始 region-route entry 列表
    synced_at       INTEGER NOT NULL DEFAULT 0,
    UNIQUE(controller_id, service_group, cluster, namespace, kind, resource_name)
);
```

设计要点：
- `region_route_cache` 用 `UNIQUE` 约束做 upsert（`INSERT OR REPLACE`），不需要先查再写
- `entries_json` 存整个 entry 列表的 JSON 序列化，避免过度范式化——RegionRoute 是只读缓存，不需要逐字段查询
- `controllers.online` 在每次同步任务执行时更新，反映心跳检查结果

## 配置（edgion-center.yaml）

```yaml
database:
  enabled: true
  sqlite_path: data/center.db  # 相对路径，以 work_dir 为基准
  sync_interval_secs: 60       # 后台同步任务间隔，单位秒
```

`enabled: false` 时所有 Admin API 返回 503，同步任务不启动。

## 后台同步任务（`db/sync.rs::run_sync_task`）

同步循环在 Center 启动时以独立 task 运行，每 `sync_interval_secs` 秒触发一次。

**同步流程（4 步）：**

1. 读取所有已注册 controller（从内存 registry，非 DB）
2. **批量 upsert** 所有 controller 记录到 `controllers` 表（含当前 online/offline 状态）
3. 对 **online** controller 调用 `GET /api/v1/cluster-region-pms`，获取所有 ClusterRegionRoute PluginMetaData 列表
4. 并发拉取详情，批量 upsert 到 `region_route_cache`

> 注：步骤 3-4 在 RegionRoute PM 重构后改用新的 `cluster-region-pms` / `service-region-pms` 端点，旧的 `region-routes` 端点已删除。

**为什么只同步 online controller：**
- Offline controller 无法响应 HTTP，不必尝试
- 其历史缓存数据在上次在线时已写入 DB，保持不变直到下次上线

**Admin API 可触发即时同步：**
`POST /api/v1/center/admin/sync` → 返回 `{"controller_count": N}`，fire-and-forget，不等待同步完成

## 离线回退（`api/region_pm_handlers.rs`）

`GET /api/v1/center/cluster-region-pms` 的处理逻辑：

1. 对所有 **online** controller：并发拉取 `GET /api/v1/cluster-region-pms`
2. 对 **offline** controller 或在线但拉取失败的 controller：从 `region_route_cache` 读历史数据
3. 合并响应：条目含 `is_cached: bool` 和 `last_synced_at: Option<i64>` 标识数据来源

**前端意义：** 带 `is_cached: true` 的 controller 条目会显示"缓存"标签，让用户知道数据可能过时。

## Admin API

所有端点在 `database.enabled = false` 时返回 503。

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/center/admin/controllers` | 列出 DB 中持久化的 controller 记录（含离线记录） |
| `DELETE` | `/api/v1/center/admin/controllers/{id}` | 删除指定 controller 记录及其所有缓存数据 |
| `DELETE` | `/api/v1/center/admin/cache` | 清空所有 `region_route_cache` 行（保留 controller 记录） |
| `POST` | `/api/v1/center/admin/sync` | 立即触发一次同步，返回 controller 数量 |

**典型运维场景：**
- controller 永久下线 → `DELETE /controllers/{id}` 清除其历史数据
- 缓存数据疑似污染 → `DELETE /cache` 清空，等下次同步重建
- 调试同步是否正常 → `POST /sync` 手动触发，再 `GET /controllers` 验证 `last_seen_at`

## Docker 构建注意事项

`rusqlite bundled` 在 **编译期** 完成所有工作：
- `Dockerfile.builder`（rust:bookworm + gcc）：无需额外包，gcc 已包含
- `Dockerfile.runtime`（ubuntu:24.04 + libssl3）：无需安装 `libsqlite3`，SQLite 已静态链接进二进制

唯一的影响是首次全量编译多约 30 秒（C 代码编译），之后 cargo 缓存层命中不会重复编译。

## 源码位置

| 模块 | 路径 |
|------|------|
| DB 初始化 & 连接管理 | `src/core/center/db/mod.rs` |
| 后台同步任务 | `src/core/center/db/sync.rs` |
| Schema 建表 SQL | `src/core/center/db/schema.rs`（或 `mod.rs` 内联） |
| Admin API handlers | `src/core/center/api/admin_handlers.rs` |
| RegionRoute detail handler（含离线回退） | `src/core/center/api/region_route_handlers.rs` |
| Center 配置结构 | `src/core/center/config.rs`（`DatabaseConfig` 字段） |
