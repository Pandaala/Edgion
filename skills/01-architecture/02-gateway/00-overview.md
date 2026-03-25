---
name: gateway-overview
description: edgion-gateway 总体架构：双运行时模型、配置同步管线、Pingora 代理层、模块划分、关键数据流。
---

# Gateway 总体架构

edgion-gateway 是 Edgion 的数据面，基于 Cloudflare Pingora 构建的高性能反向代理。它通过 gRPC 从 Controller 接收 Kubernetes 风格的资源配置，并将其转化为运行时代理行为。

## 双运行时模型

Gateway 进程内并存两个运行时：

| 运行时 | 职责 | 生命周期 |
|--------|------|---------|
| **Tokio** | gRPC Watch/List、配置处理、辅助服务（Admin API、Metrics API、健康检查）、所有 async 任务 | 启动时创建，Phase 2 前移入后台线程，以 `std::future::pending` 永久挂起保持存活 |
| **Pingora** | 代理主循环：TCP 连接接收、TLS 握手、HTTP/gRPC 请求处理、上游连接复用 | 由 `Server::run_forever()` 在主线程阻塞运行 |

两个运行时通过 **ArcSwap** 原子指针共享配置数据——Tokio 侧写入，Pingora 侧无锁读取。这种设计使得配置热更新不会阻塞代理请求处理。

## 配置同步管线

```
Controller gRPC ──> ConfigSyncClient
                         │
                         ▼
                    ClientCache<T>  (per resource kind)
                         │
                    event_dispatch (apply_change / set_ready)
                         │
                         ▼
                    ConfHandler<T>  (full_set / partial_update)
                         │
                         ▼
                Runtime Stores (ArcSwap atomic swap)
```

关键组件说明：

- **ConfigSyncClient** (`conf_sync/conf_client/grpc_client.rs`)：管理与 Controller 的 gRPC 连接，负责 List/Watch 调用、版本跟踪、断线重连。通过 `get_server_info()` 获取 endpoint_mode 和 supported_kinds。
- **ClientCache\<T\>** (`conf_sync/cache_client/cache.rs`)：每种资源类型一个缓存实例，内部持有 `CacheData<T>`（RwLock 保护）和 gRPC 客户端引用。实现了 `DynClientCache` trait 以支持类型擦除的统一调度。
- **event_dispatch** (`conf_sync/cache_client/event_dispatch.rs`)：实现 `CacheEventDispatch<T>` trait，将 gRPC 事件（InitAdd/EventAdd/EventUpdate/EventDelete）应用到 CacheData，并压缩事件批次。
- **ConfHandler\<T\>** (`core/common/conf_sync/traits.rs`)：资源处理器 trait，每种资源类型一个实现。提供 `full_set()` 和 `partial_update(add, update, remove)` 两种回调，负责将资源写入运行时存储。
- **ConfigClient** (`conf_sync/conf_client/config_client.rs`)：聚合所有资源类型的 ClientCache，提供 `list_gateways()`、`get_gateway_class()`、`get_edgion_gateway_config()` 等类型安全的查询接口，以及 `is_ready()` 就绪检查。

## Pingora 代理层

### ConnectionFilter（TCP 层）

`StreamPluginConnectionFilter` 实现 Pingora 的 `ConnectionFilter` trait，在 TLS 握手和 HTTP 解析之前对原始 TCP 连接执行过滤。通过 Gateway annotation `edgion.io/edgion-stream-plugins` 引用 `EdgionStreamPlugins` 资源，从全局 `StreamPluginStore`（ArcSwap）读取最新配置，支持热重载。每次连接调用 `should_accept(addr)` 执行所有 StreamPlugin 链。

### ProxyHttp（HTTP/gRPC 层）

`EdgionHttpProxy` 实现 Pingora 的 `ProxyHttp` trait，处理 HTTP 和 gRPC 请求的完整生命周期。以 `EdgionHttpContext` 为每请求状态载体，经过 early_request_filter → request_filter → upstream_peer → connected_to_upstream → upstream_response_filter → upstream_response_body_filter → response_filter → logging 八个阶段。

### TCP/TLS/UDP 路由

除 HTTP/gRPC 外，Gateway 还支持：
- **TCPRoute**：TCP 四层代理，按端口和 SNI 匹配
- **TLSRoute**：TLS passthrough 代理，按 SNI 匹配后直接转发加密流量
- **UDPRoute**：UDP 代理

每种协议类型有独立的 `routes_mgr`（路由管理器）和 `route_table`（路由表），均使用 ArcSwap 实现无锁热更新。

## 模块布局

```
src/core/gateway/
├── api/           # Admin API 服务器 (:5900)，提供运行时状态查询
├── backends/      # 后端管理
│   ├── discovery/ # 后端发现：Endpoint / EndpointSlice / Service 三种存储
│   ├── health/    # 健康检查：annotation 驱动的探针、状态存储、Manager 调度
│   ├── policy/    # BackendTLSPolicy 处理
│   ├── preload.rs # 启动时预加载所有路由的负载均衡器
│   └── validation.rs
├── cache/         # LRU 缓存
├── cli/           # 启动入口 (EdgionGatewayCli::run())
│   ├── config.rs  # EdgionGatewayConfig TOML 配置结构
│   ├── pingora.rs # Pingora Server 创建与运行 (两阶段)
│   └── log_config.rs
├── conf_sync/     # 配置同步
│   ├── cache_client/ # ClientCache<T>、CacheData、event_dispatch
│   └── conf_client/  # ConfigSyncClient (gRPC)、ConfigClient (聚合查询)
├── config/        # 资源配置处理
│   ├── edgion_gateway/ # EdgionGatewayConfig ConfHandler
│   └── gateway_class/  # GatewayClass ConfHandler
├── lb/            # 负载均衡
│   ├── backend_selector/ # 后端选择器 (加权轮询)
│   ├── selection/        # 策略实现：ConsistentHash、EWMA、LeastConn
│   ├── ewma/             # EWMA 指标收集
│   ├── leastconn/        # 最少连接状态与清理器
│   ├── lb_policy/        # LB 策略配置与类型
│   └── runtime_state/    # 运行时 LB 状态 (increment/decrement/update_ewma)
├── link_sys/      # 外部系统集成 (LinkSys)
│   ├── providers/ # Elasticsearch、Etcd、Redis、Webhook、LocalFile
│   └── runtime/   # ConfHandler、DataSender
├── observe/       # 可观测性
│   ├── access_log/  # AccessLog：entry 定义、logger、store
│   └── metrics/     # Prometheus 指标、全局计数器
│   └── logs/        # 系统日志、SSL/TCP/TLS/UDP 日志
├── plugins/       # 插件系统
│   ├── http/      # HTTP 插件集合
│   ├── stream/    # Stream 插件 (ConnectionFilter bridge、TLS route 插件)
│   └── runtime/   # 插件执行引擎 (PluginRuntime、SessionAdapter)
├── routes/        # 多协议路由
│   ├── http/      # HTTPRoute：匹配引擎 (radix + regex)、路由管理器、ProxyHttp 实现
│   ├── grpc/      # GRPCRoute：匹配与上游处理
│   ├── tcp/       # TCPRoute
│   ├── tls/       # TLSRoute
│   └── udp/       # UDPRoute
├── runtime/       # Pingora 运行时
│   ├── server/    # GatewayBase、listener_builder、error_response、server_header
│   ├── matching/  # Gateway 匹配、TLS 证书匹配 (ArcSwap)
│   ├── store/     # GatewayStore、GatewayConfigStore、PortGatewayInfoStore (ArcSwap)
│   ├── handler.rs # Gateway 资源 ConfHandler 实现
│   └── gateway_info.rs
├── services/      # 附加服务
│   └── acme/      # ACME 证书自动签发 (challenge store)
└── tls/           # TLS 证书管理
    ├── boringssl/ # BoringSSL 后端
    ├── openssl/   # OpenSSL 后端
    ├── runtime/   # TLS 回调 (SNI 匹配、证书选择)
    ├── store/     # 证书存储
    └── validation/ # 证书校验
```

## 关键数据流

### 配置更新流

```
Controller 推送资源变更
  → ConfigSyncClient 接收 gRPC Watch 事件
    → ClientCache<T>.apply_change() 更新 CacheData
      → 事件压缩 (compress_events)
        → ConfHandler<T>.full_set() 或 partial_update()
          → 写入运行时存储 (ArcSwap.store())
            → Pingora 代理侧下次读取即生效 (无锁)
```

### 请求处理流

```
客户端 TCP 连接
  → ConnectionFilter: StreamPlugins 过滤 (IP 限制等)
    → TLS 握手: SNI 匹配证书 (GatewayTlsMatcher via ArcSwap)
      → HTTP 解析
        → early_request_filter: ACME challenge、超时设置
          → request_filter: 元数据提取、路由匹配、插件执行
            → upstream_peer: 后端选择、LB 策略
              → 代理请求到上游
                → upstream_response_filter/body_filter: 响应处理
                  → response_filter: 异步响应插件
                    → logging: 指标记录 + AccessLog
```
