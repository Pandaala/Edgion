---
name: architecture
description: 架构导航 skill。按 bin 类型（Controller/Gateway/Ctl）和资源类型组织，涵盖通用约定、各 bin 架构、gRPC 同步、资源处理。
---

# 01 架构与核心功能

> Edgion 是基于 Pingora 的 Kubernetes Gateway，采用 Controller–Gateway 分离架构。

## 整体架构

Edgion 是单 Crate、三 bin 的项目。**Controller 决策，Gateway 执行，Ctl 运维。**

```
  用户 / K8s API                                            客户端流量
       │                                                         │
       ▼                                                         ▼
┌──────────────────────────────────────┐    gRPC     ┌───────────────────────────────────────┐
│         edgion-controller            │  Watch/List │          edgion-gateway                │
│         (控制面)                     │ ──────────► │          (数据面)                      │
│                                      │             │                                       │
│  ConfCenter (File/K8s)               │             │  ConfigSyncClient → ClientCache       │
│  └► Workqueue (per-kind)             │             │                     └► ConfHandler     │
│     └► ResourceProcessor            │             │                                       │
│        (validate/preparse/parse)     │             │  Pingora Server                       │
│        └► ServerCache ──────────► gRPC 推送 ──────►│  ├─ ConnectionFilter (StreamPlugins)   │
│                                      │             │  ├─ ProxyHttp (HTTP/gRPC)              │
│  跨资源依赖                          │             │  │  ├─ route match + plugins           │
│  ├─ SecretRefManager                 │             │  │  ├─ upstream_peer + LB              │
│  ├─ ServiceRefManager               │             │  │  └─ logging (AccessLog + Metrics)   │
│  ├─ CrossNamespaceRefManager         │             │  └─ TCP/TLS/UDP Routes                │
│  └─ RequeueChain                     │             │                                       │
│                                      │             │  TLS 子系统                            │
│  Admin API (:5800)                   │             │  ├─ 下游 TLS (客户端→网关)             │
│  ConfigSyncServer (:50051)           │             │  └─ 上游 TLS (网关→后端 mTLS)         │
│  ACME Service (可选, 仅 Leader)      │             │                                       │
└──────────────────────────────────────┘             │  Admin API (:5900)                     │
       ▲                                             │  Metrics  (:5901)                      │
       │ HTTP API                                    └───────────────────────────────────────┘
┌──────┴───────┐
│ edgion-ctl   │
│ (CLI 工具)   │
│              │
│ apply/delete │
│ get/reload   │
│              │
│ 3 种 target: │
│ center/server│
│ /client      │
└──────────────┘
```

- **edgion-controller**（控制面）：从 K8s API 或本地文件接收资源，经校验/预解析/处理后通过 gRPC 推送给 Gateway。负责状态回写、跨资源依赖管理、ACME 证书签发。
- **edgion-gateway**（数据面）：基于 Pingora 的高性能代理，从 Controller 接收配置后处理实际流量。支持 HTTP/gRPC/TCP/TLS/UDP，具备插件系统和多种负载均衡策略。
- **edgion-ctl**（CLI 工具）：通过 HTTP API 与 Controller/Gateway 交互，提供资源 CRUD、状态查询和运维操作。

## 阅读指引

| 你想了解… | 从这里开始 |
|-----------|-----------|
| 项目全貌、代码组织、三 bin 共用约定 | [00-common/](00-common/SKILL.md) |
| Controller 内部如何处理资源 | [01-controller/](01-controller/SKILL.md) |
| Gateway 如何处理请求 | [02-gateway/](02-gateway/SKILL.md) |
| Controller 和 Gateway 如何同步 | [03-controller-gateway-link/](03-controller-gateway-link/SKILL.md) |
| edgion-ctl 命令和 target 模式 | [04-ctl/](04-ctl/SKILL.md) |
| 某种具体资源的完整处理链路 | [05-resources/](05-resources/SKILL.md) |
| 排查路由不匹配 | [02-gateway/03-routes/00-route-matching.md](02-gateway/03-routes/00-route-matching.md) |
| 排查 TLS 证书问题 | [02-gateway/04-tls/](02-gateway/04-tls/) |
| 排查 gRPC 同步问题 | [03-controller-gateway-link/00-overview.md](03-controller-gateway-link/00-overview.md) |

---

## 目录总览

### [00-common/](00-common/SKILL.md) — 通用约定

三个 bin 共同遵守的基础约定：项目总览、命令行/目录/配置路径、Core 模块分层、资源系统（define_resources! 宏）。

### [01-controller/](01-controller/SKILL.md) — edgion-controller 架构

控制面内部架构，按模块拆分：

| 模块 | 关键词 |
|------|--------|
| 总体架构 | ConfMgr 门面、ProcessorRegistry |
| 启动/关闭 | Leader 选举、HA 模式、事件驱动主循环 |
| Admin API | :5800 端口、CRUD、health/ready |
| 配置中心 | ConfCenter trait、FileSystem / Kubernetes 两种实现 |
| Workqueue | 去重、指数退避、dirty requeue |
| ResourceProcessor | 11 步处理流水线、23 种 Handler |
| Requeue | 跨资源联动、TriggerChain 环检测 |
| CacheServer | ServerCache + EventStore、gRPC 数据源 |
| ACME | Let's Encrypt 自动证书、仅 Leader |

### [02-gateway/](02-gateway/SKILL.md) — edgion-gateway 架构

数据面内部架构，按子系统拆分：

| 子系统 | 关键词 |
|--------|--------|
| 总体架构 | 双运行时模型（Tokio + Pingora） |
| 启动/关闭 | 14 步启动序列、两阶段 Pingora 集成 |
| Pingora 生命周期 | 7 阶段 ProxyHttp 回调、ConnectionFilter |
| [路由](02-gateway/03-routes/) | 多级匹配流水线、HTTP/gRPC/TCP/TLS/UDP 各自引擎 |
| [TLS](02-gateway/04-tls/) | 下游/上游 TLS、SNI 匹配、BoringSSL/OpenSSL |
| 插件系统 | 4 阶段执行、28 个 HTTP 插件、条件执行 |
| 负载均衡 | RoundRobin/EWMA/LeastConn/ConsistentHash/Weighted |
| 后端 | Service/EndpointSlice 发现、健康检查 |
| 可观测性 | AccessLog（零拷贝 JSON）、Prometheus Metrics |
| LinkSys | ES/Redis/Etcd/Webhook/File 外部集成 |
| 运行时存储 | Gateway/Route 配置、ArcSwap 原子切换 |

### [03-controller-gateway-link/](03-controller-gateway-link/SKILL.md) — 双向 gRPC 同步

ConfigSync 协议（GetServerInfo / List / Watch / WatchServerMeta），Controller 侧 ConfigSyncServer + Gateway 侧 ConfigSyncClient 的实现。

### [04-ctl/](04-ctl/SKILL.md) — edgion-ctl 架构

CLI 工具：3 种 target 模式（center/server/client）、apply/delete/get/reload 子命令。

### [05-resources/](05-resources/SKILL.md) — 资源架构

通用处理流程 + 20 种资源的功能点、特殊处理、跨资源关联：

| 分类 | 资源 |
|------|------|
| 核心配置 | Gateway, GatewayClass, EdgionGatewayConfig |
| 路由 | HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute |
| 安全/策略 | EdgionTls, Secret, ReferenceGrant, BackendTLSPolicy |
| 插件/扩展 | EdgionPlugins, EdgionStreamPlugins, PluginMetaData |
| 后端/服务 | Service, EndpointSlice, Endpoint |
| 自动化 | EdgionAcme |
| 基础设施 | LinkSys |

### [06-gateway-api.md](06-gateway-api.md) — Gateway API 合规性

Gateway API v1.4.0 支持范围、一致性测试、Edgion 扩展点、有意偏差。

---

## 旧版文件

原有文件已备份到 `_01-architecture-old/`，内容逐步迁移到新结构后将删除。
