---
name: controller-admin-api
description: edgion-controller Admin HTTP API（Axum :5800）：全部端点、AdminState、健康检查、CRUD 与 ConfigServer 接口。
---

# Admin API (:5800)

Controller 通过 Axum 提供 HTTP Admin API，监听端口 5800，供 edgion-ctl 命令行工具和运维操作使用。路由在 `create_admin_router()` 中统一注册。

## AdminState 共享状态

所有 handler 通过 Axum 的 `State<Arc<AdminState>>` 共享状态：

```rust
pub struct AdminState {
    pub conf_mgr: Arc<ConfMgr>,
    pub schema_validator: Arc<SchemaValidator>,
}
```

`AdminState` 提供两层数据访问方法：

| 方法前缀 | 数据源 | 用途 |
|----------|--------|------|
| `center_*` | ConfCenter（存储层） | `/api/v1/...` 端点使用，直接读写底层存储 |
| `cache_*` | ConfigSyncServer（ServerCache） | `/configserver/...` 端点使用，读取处理后的缓存 |

辅助方法：

- `config_sync_server()` — 获取 `Arc<ConfigSyncServer>`，未就绪时返回 `503`
- `is_ready()` — 检查 ConfigSyncServer 是否存在
- `is_k8s_mode()` — 是否为 Kubernetes 模式（K8s 模式下跳过本地 Schema 校验）

## 全部端点列表

### 健康与状态

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/health` | 存活探测（Liveness）。服务启动即返回 `200`，响应 `{"success":true,"data":"OK"}` |
| GET | `/ready` | 就绪探测（Readiness）。ConfigSyncServer 就绪时返回 `200`，否则返回 `503` |
| GET | `/api/v1/server-info` | 获取当前 `server_id` 和就绪状态。reload 后 `server_id` 会变更，可用于确认 reload 生效 |

### Reload

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/v1/reload` | 触发全量重载。异步执行：停止当前 controller -> 清空 PROCESSOR_REGISTRY -> 创建新 ConfigSyncServer（新 server_id）-> 重启 controller（完整 Init -> InitApply -> InitDone 流程）。Gateway 客户端检测到 server_id 变化后自动 re-list |

### 命名空间级资源 CRUD

所有端点从 **ConfCenter 存储层** 读写。请求体支持 JSON 和 YAML 格式（优先尝试 JSON 解析）。

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/namespaced/{kind}` | 列出某 kind 的所有命名空间下资源 |
| GET | `/api/v1/namespaced/{kind}/{namespace}` | 列出某 kind 在指定命名空间下的资源 |
| POST | `/api/v1/namespaced/{kind}/{namespace}` | 创建命名空间级资源 |
| GET | `/api/v1/namespaced/{kind}/{namespace}/{name}` | 获取单个命名空间级资源 |
| PUT | `/api/v1/namespaced/{kind}/{namespace}/{name}` | 更新命名空间级资源 |
| DELETE | `/api/v1/namespaced/{kind}/{namespace}/{name}` | 删除命名空间级资源 |

路径参数中的 `{kind}` 为资源类型名称（如 `HTTPRoute`、`Gateway`），通过 `parse_kind()` 进行大小写不敏感匹配，未知 kind 返回 `400`。

非 K8s 模式下，创建和更新操作会：
1. 使用 `SchemaValidator` 进行 JSON Schema 校验（K8s 模式跳过，由 API Server 负责）
2. 自动调用 `next_resource_version()` 分配递增的 `resourceVersion`

### 集群级资源 CRUD

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/cluster/{kind}` | 列出某 kind 的所有集群级资源 |
| POST | `/api/v1/cluster/{kind}` | 创建集群级资源 |
| GET | `/api/v1/cluster/{kind}/{name}` | 获取单个集群级资源 |
| PUT | `/api/v1/cluster/{kind}/{name}` | 更新集群级资源 |
| DELETE | `/api/v1/cluster/{kind}/{name}` | 删除集群级资源 |

集群级资源（如 GatewayClass）没有 namespace 维度。handler 内部通过 `is_kind_cluster_scoped()` 校验 kind 是否确实是集群级的，不匹配则返回 `400`。

### ConfigServer 端点

供 `edgion-ctl --target server` 使用，从 **ConfigSyncServer 缓存层**（ServerCache）读取数据。返回的是经过 ResourceProcessor 处理后的资源快照，格式与 Gateway 的 `/configclient/` API 兼容。

| 方法 | 路径 | 查询参数 | 说明 |
|------|------|----------|------|
| GET | `/configserver/{kind}/list` | 无 | 列出某 kind 的所有缓存资源 |
| GET | `/configserver/{kind}` | `namespace`（可选）、`name`（必须） | 获取单个缓存资源。命名空间级资源需传 namespace |

响应格式示例：

```json
// 列表响应
{"success": true, "data": [...], "count": 5}

// 单资源响应
{"success": true, "data": {"metadata": {...}, "spec": {...}}}

// 错误响应
{"success": false, "error": "HTTPRoute not found"}
```

### ACME 服务

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/v1/services/acme/{namespace}/{name}/trigger` | 手动触发指定 ACME 资源的证书签发/续期。重置重试计数器并重新评估资源 |

## Health 与 Readiness 语义

| 探测 | 端点 | 语义 | K8s 用途 |
|------|------|------|----------|
| Liveness | `/health` | 进程存活即返回 200 | `livenessProbe` — 失败则重启 Pod |
| Readiness | `/ready` | ConfigSyncServer 初始化完成且所有 Processor 就绪后返回 200 | `readinessProbe` — 未就绪时从 Service 端点摘除 |

Readiness 的判定通过 `AdminState::is_ready()` -> `ConfMgr::is_ready()` -> `ConfCenter::is_ready()`，最终检查 ConfigSyncServer 是否已创建。

## 与 edgion-ctl 的交互

edgion-ctl 支持三种 `--target` 模式，对应不同端点：

| target 模式 | 访问端点 | 数据源 | 说明 |
|-------------|----------|--------|------|
| `center` | `/api/v1/namespaced/...` 和 `/api/v1/cluster/...` | ConfCenter 存储层 | 查看/修改原始配置 |
| `server` | `/configserver/...` | ConfigSyncServer 缓存 | 查看处理后的配置快照（与 Gateway 看到的一致） |
| `client` | Gateway 的 `/configclient/...` | Gateway 本地缓存 | 查看 Gateway 实际使用的配置 |

这三层视图帮助运维人员定位配置同步链路上的问题：存储层有但缓存没有说明 Processor 处理异常；缓存有但 Gateway 没有说明 gRPC 同步异常。

## 错误映射

`ConfWriterError` 到 HTTP 状态码的映射：

| ConfWriterError 变体 | HTTP 状态码 |
|---------------------|-------------|
| `NotFound` | 404 Not Found |
| `AlreadyExists` | 409 Conflict |
| `ValidationError` | 400 Bad Request |
| `PermissionDenied` | 403 Forbidden |
| `Conflict` | 409 Conflict |
| `ParseError` | 400 Bad Request |
| `IOError` | 500 Internal Server Error |
| `KubeError` | 500 Internal Server Error |
| `InternalError` | 500 Internal Server Error |

## 启动方式

Admin API 支持两种启动模式：

```rust
// 普通启动
api::serve(conf_mgr, schema_validator, addr).await?;

// 带优雅关闭
api::serve_with_shutdown(conf_mgr, schema_validator, addr, shutdown_signal).await?;
```

`serve_with_shutdown` 使用 Axum 的 `with_graceful_shutdown`，在收到关闭信号后停止接受新连接并等待已有请求完成。
