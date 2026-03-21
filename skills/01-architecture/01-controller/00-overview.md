---
name: controller-overview
description: edgion-controller 总体架构：ConfMgr 门面、双后端、Workqueue + ResourceProcessor 流水线、模块布局。
---

# Controller 总体架构

edgion-controller 是 Edgion 的控制面进程，基于 Tokio 多线程运行时运行。它负责从配置源（文件系统或 Kubernetes API）接收资源变更，经过校验与处理后，通过 gRPC 将最终配置推送给 Gateway 数据面。

## ConfMgr 门面模式

`ConfMgr` 是配置管理的统一入口，采用经典的 **Facade 模式**。它内部持有一个 `Arc<dyn ConfCenter>` trait 对象，将所有 CRUD 和生命周期操作委托给具体后端实现。

```rust
pub struct ConfMgr {
    conf_center: Arc<dyn ConfCenter>,
}
```

`ConfCenter` 是一个超级 trait，由两个子 trait 组合而成：

```text
ConfCenter (super trait)
├── CenterApi        — CRUD 操作（get_one, set_one, delete_one, list ...）
└── CenterLifeCycle  — 生命周期（start, is_ready, request_reload ...）
```

`ConfMgr::create(config)` 工厂方法根据配置枚举 `ConfCenterConfig` 选择后端：

| 配置变体 | 创建的后端 | 说明 |
|----------|-----------|------|
| `ConfCenterConfig::FileSystem(fs_config)` | `FileSystemCenter` | 从本地目录读写 YAML 文件 |
| `ConfCenterConfig::Kubernetes(k8s_config)` | `KubernetesCenter` | 通过 K8s API 监听 CRD 和原生资源 |

创建后，`ConfMgr` 还会注册 `CrossNsRevalidationListener`，确保 ReferenceGrant 变更时能触发受影响路由的重新校验。

## 双后端实现

### FileSystemCenter

- 从指定目录递归读取 YAML 配置文件
- 适用于无 Kubernetes 环境的独立部署或本地开发
- 通过 `CenterApi` trait 提供文件级 CRUD

### KubernetesCenter

- 使用 kube-rs 客户端连接 K8s API Server
- 监听 Edgion CRD（如 HTTPRoute、Gateway 等）以及原生资源（Secret、Service、EndpointSlice 等）
- 支持 HA 模式下的 Leader Election
- 变更事件通过 Informer/Watcher 推入对应资源的 Workqueue

## Workqueue + ResourceProcessor 架构

每种资源类型（kind）拥有独立的 **Workqueue** 和 **ResourceProcessor** 实例，形成一条完整的事件处理流水线。

### Workqueue

Go controller-runtime 风格的工作队列，核心特性：

- **去重**：同一 key 在队列中只存在一次
- **指数退避**：失败重试时延迟递增
- **延迟入队**：跨资源 requeue 使用内置 DelayQueue 实现合并
- **TriggerChain**：级联路径追踪与循环检测（类似 X-Forwarded-For）
- **脏重入队**：处理期间收到的新入队请求不会丢失

### ResourceProcessor\<T\>

泛型处理器，核心组成：

| 组件 | 职责 |
|------|------|
| `ServerCache<T>` | 资源的内存缓存，处理后的对象存于此处 |
| `Workqueue` | 接收事件 key 并驱动处理循环 |
| `ProcessorHandler<T>` | trait，定义资源特定的处理逻辑（on_init、on_apply、on_delete） |
| `HandlerContext` | 传递给 handler 的上下文，提供 requeue、缓存访问等能力 |

处理结果类型 `WorkItemResult<K>`：

```rust
enum WorkItemResult<K> {
    Processed { obj: K, status_changed: bool },
    Deleted { key: String },
    Skipped,
}
```

每种资源都有对应的 Handler 实现，例如 `HttpRouteHandler`、`GatewayHandler`、`SecretHandler` 等，统一注册到 `ProcessorRegistry`。

## ProcessorRegistry 全局单例

`PROCESSOR_REGISTRY` 是一个 `LazyLock<ProcessorRegistry>` 全局单例，提供处理器的集中管理：

```rust
pub static PROCESSOR_REGISTRY: LazyLock<ProcessorRegistry> = LazyLock::new(ProcessorRegistry::new);
```

核心能力：

| 方法 | 说明 |
|------|------|
| `register(processor)` | 启动时注册处理器 |
| `get(kind)` | 按 kind 名称获取处理器 |
| `is_all_ready()` | 检查所有处理器是否就绪（空注册表返回 false） |
| `not_ready_kinds()` | 返回尚未就绪的 kind 列表 |
| `wait_kinds_ready(kinds, timeout)` | 分阶段启动时等待指定 kind 就绪 |
| `all_watch_objs(no_sync_kinds)` | 收集 WatchObj 供 ConfigSyncServer 注册（过滤不需同步的 kind） |
| `requeue(kind, key)` | 跨资源立即重入队 |
| `requeue_with_chain(kind, key, chain)` | 带 TriggerChain 的延迟重入队（合并+循环检测） |
| `requeue_all()` | Leader 切换时全量重入队，触发状态 reconciliation |
| `clear_registry()` | 清空所有处理器及全局状态（ListenerPortManager、ServiceRefManager、ReferenceGrantStore 等） |

## 模块布局

```text
src/core/controller/
├── api/                        # Admin REST API (Axum, :5800)
│   ├── mod.rs                  # 路由注册与健康检查 handler
│   ├── types.rs                # AdminState、ApiResponse、ListResponse
│   ├── common.rs               # parse_kind、validate_resource、错误映射
│   ├── namespaced_handlers.rs  # 命名空间级资源 CRUD
│   ├── cluster_handlers.rs     # 集群级资源 CRUD
│   └── configserver_handlers.rs # ConfigServer 缓存查询（供 edgion-ctl）
├── cli/                        # 启动入口、命令行参数解析、初始化流程
├── conf_mgr/                   # 配置管理器核心
│   ├── manager.rs              # ConfMgr 门面
│   ├── processor_registry.rs   # ProcessorRegistry 全局单例
│   ├── schema_validator.rs     # JSON Schema 校验
│   ├── conf_center/            # 配置存储后端
│   │   ├── traits.rs           # ConfCenter / CenterApi / CenterLifeCycle trait
│   │   ├── file_system/        # FileSystemCenter 实现
│   │   └── kubernetes/         # KubernetesCenter 实现
│   └── sync_runtime/           # 共享同步运行时
│       ├── workqueue.rs        # Workqueue（去重、退避、TriggerChain）
│       ├── shutdown.rs         # ShutdownHandle 优雅关闭
│       ├── metrics.rs          # 运行时指标
│       └── resource_processor/ # ResourceProcessor 框架
│           ├── processor.rs    # ResourceProcessor<T> 核心实现
│           ├── handler.rs      # ProcessorHandler<T> trait
│           ├── context.rs      # HandlerContext
│           ├── handlers/       # 每种资源的 Handler 实现
│           ├── ref_grant/      # 跨命名空间引用校验（ReferenceGrant）
│           ├── secret_utils/   # Secret 全局存储与引用管理
│           ├── service_ref.rs  # Service 引用追踪
│           ├── listener_port_manager.rs  # Gateway Listener 端口冲突检测
│           ├── attached_route_tracker.rs # 路由挂载追踪
│           ├── gateway_route_index.rs    # Gateway↔Route 索引
│           ├── namespace_store.rs        # Namespace 存储
│           └── status_utils.rs           # 状态条件工具函数
├── conf_sync/                  # gRPC 配置同步
│   ├── cache_server/           # ServerCache — 内存缓存层
│   └── conf_server/            # ConfigSyncServer — gRPC 服务端
├── observe/                    # 可观测性（指标、追踪）
└── services/                   # 附加服务
    └── acme/                   # ACME 证书自动签发与续期
```

## 数据流总览

完整的数据流从事件源到 Gateway 同步：

```text
┌─────────────────┐     ┌─────────────────┐
│ K8s Informer /  │     │  FileSystem     │
│ Watcher         │     │  Watcher        │
└────────┬────────┘     └────────┬────────┘
         │ 事件(Add/Update/Delete)│
         └──────────┬────────────┘
                    ▼
            ┌───────────────┐
            │   Workqueue   │  每个 kind 独立队列
            │  (去重+退避)  │  支持跨资源 requeue
            └───────┬───────┘
                    ▼
         ┌─────────────────────┐
         │ ResourceProcessor<T>│
         │  ├─ on_init         │  初始化加载
         │  ├─ on_apply        │  新增/更新处理
         │  └─ on_delete       │  删除处理
         └─────────┬───────────┘
                   ▼
           ┌──────────────┐
           │ ServerCache  │  内存缓存（处理后的资源）
           └──────┬───────┘
                  ▼
        ┌──────────────────┐
        │ ConfigSyncServer │  gRPC 服务端
        │  (Watch/List)    │  检测变更并推送增量
        └────────┬─────────┘
                 ▼
          ┌────────────┐
          │  Gateway   │  数据面接收配置
          └────────────┘
```

关键流转细节：

1. **事件源**：K8s 模式下由 Informer/Watcher 产生事件；文件系统模式下由文件变更触发
2. **Workqueue**：接收资源 key，去重后按序出队；失败时指数退避重试
3. **ResourceProcessor**：从队列取 key，调用对应 `ProcessorHandler` 处理；处理结果写入 `ServerCache`
4. **ServerCache**：存储处理后的资源对象；变更通过 `CacheEventDispatch` 通知 ConfigSyncServer
5. **ConfigSyncServer**：维护每个 Gateway 客户端的 Watch 流，检测 server_id 变化触发全量重列，正常情况下推送增量变更
6. **跨资源联动**：处理器通过 `HandlerContext::requeue` 或 `ProcessorRegistry::requeue_with_chain` 触发其他 kind 的重新处理，TriggerChain 防止无限循环
