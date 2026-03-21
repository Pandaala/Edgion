---
name: resource-flow
description: 资源通用处理流程：从 K8s/文件系统变更到 Controller 处理再到 Gateway 运行时生效的完整链路。
---

# 资源通用处理流程

> 每种资源都遵循相同的基础流程从来源到最终生效。本文描述通用流程，各资源的特殊处理见各自文档。

## 阶段一：资源来源

资源变更事件来自两种来源：

- **Kubernetes 模式**：K8s API Server → Reflector（长连接 Watch）→ 产出 Added/Modified/Deleted 事件
- **FileSystem 模式**：本地 YAML 文件目录 → inotify/kqueue 监听 → 文件变更转化为等效的 Added/Modified/Deleted 事件

两种模式产出的事件格式统一，后续处理完全相同。

## 阶段二：Controller 处理（ResourceProcessor）

事件进入 Controller 后经过以下处理链：

```
Event → Workqueue（去重、合并）→ Worker 出队 → ResourceProcessor 处理
```

ResourceProcessor 对每个资源执行以下有序步骤：

1. **filter()** — 过滤不属于本控制器的资源（如 GatewayClass 按 controllerName 过滤，Gateway 按 gatewayClassName 过滤）
2. **validate()** — 校验资源配置合法性，产出 `validation_errors` 列表
3. **preparse()** — 预解析，在资源类型层面构建运行时结构（如 HTTPRoute 的 `PluginRuntime`、正则预编译）。此步骤由类型自身的 `preparse()` 方法实现，不在 Handler 中
4. **parse()** — 解析引用关系：解析 Secret 引用（从 GLOBAL_SECRET_STORE 读取并填充）、注册 SecretRefManager/CrossNsRefManager 引用、解析 hostname/port、标记被拒绝的跨命名空间引用
5. **update_status()** — 基于 validate 和 parse 的结果设置 Gateway API 标准的 Status Conditions（Accepted、ResolvedRefs、Conflicted 等）
6. **check change** — 对比新旧资源 JSON，检测是否有实质性变更（包括 spec 和 status）
7. **on_change()** — 仅在有实质性变更时执行：更新依赖索引（gateway_route_index、attached_route_tracker）、触发关联资源 requeue
8. **更新 ServerCache** — 将最终的资源数据写入 ServerCache，触发 gRPC 同步

删除事件执行 **on_delete()** 替代 parse/on_change，清理所有注册的引用关系和索引。

初始化阶段完成后执行 **on_init_done()**，用于 Secret 等资源做一次性的全量替换。

## 阶段三：gRPC 同步（Controller → Gateway）

```
ServerCache 变更 → ConfigSyncServer → WatchResponse（增量）→ gRPC Stream → ConfigSyncClient → ClientCache
```

- **Watch 模式**（常态）：ServerCache 检测到变更后，通过 gRPC 双向流推送增量事件（add/update/remove），每个事件携带 `sync_version`
- **List 模式**（初始化/断线重连）：Gateway 启动或断线后通过 List 获取全量数据，然后切换回 Watch
- **no_sync_kind**：部分资源不同步到 Gateway，默认为 `["ReferenceGrant", "Secret"]`。这些资源仅在 Controller 侧处理，Gateway 不需要

## 阶段四：Gateway 接收

```
ConfigSyncClient → ClientCache 更新 → EventDispatch → ConfHandler
```

ClientCache 收到事件后，通过 EventDispatch 分发给对应资源类型注册的 `ConfHandler`：

- **full_set()** — 对应 List 模式，接收全量数据，全量重建运行时状态
- **partial_update(add, update, remove)** — 对应 Watch 模式，增量更新运行时状态

## 阶段五：Gateway 运行时生效

ConfHandler 将数据写入运行时存储：

- 路由类资源：写入 per-port 的路由管理器（`GlobalHttpRouteManagers`、`GlobalTcpRouteManagers` 等），通过 `ArcSwap` 原子切换路由表
- TLS 资源：写入 `TlsStore`，更新 SNI 证书匹配表
- 后端资源：写入后端发现存储，更新负载均衡目标列表
- 插件资源：写入插件配置存储，通过 ExtensionRef 被路由引用

请求处理时直接从运行时存储读取，无锁访问：

```
请求 → 端口监听 → 路由匹配（port → domain → path → deep match）→ 读取后端/插件配置 → 转发
```

## 跨资源联动

资源之间存在依赖关系，变更时通过 requeue 机制级联更新：

- **SecretRefManager**：Secret 变更时，requeue 所有引用该 Secret 的资源（Gateway、EdgionTls、EdgionPlugins、EdgionAcme、BackendTLSPolicy）
- **ServiceRefManager**：Service 变更时，requeue 所有引用该 Service 的路由
- **CrossNsRefManager**：ReferenceGrant 变更时，requeue 所有跨命名空间引用受影响的资源
- **GatewayRouteIndex**：Gateway 变更时，requeue 所有通过 parentRef 引用该 Gateway 的路由和 EdgionTls
- **AttachedRouteTracker**：路由变更时，更新 Gateway 的 attachedRoutes 计数，requeue Gateway 刷新 Status
- **TriggerChain**：限制 requeue 链深度（默认 3 层），防止循环

## Status 回写

Controller 在处理完成后更新资源的 Status：

- **Kubernetes 模式**：通过 K8s API status subresource 写回
- **FileSystem 模式**：写入 `.status` 文件
- 仅 Leader 节点执行 Status 回写
- Status 变更检测：仅在 Status 实际发生变化时才执行回写，避免无意义的 API 调用
