# 00 架构与核心功能

> Edgion 是基于 Pingora 的 Kubernetes Gateway，采用 Controller–Gateway 分离架构。
> 本目录包含各核心子系统的设计文档，帮助理解项目的整体设计和内部工作原理。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-overview.md](00-overview.md) | 项目总览与代码组织 | 首次接触项目、需要全局视角 |
| [01-config-center.md](01-config-center.md) | 配置中心（Controller 核心） | 修改资源处理流程、理解 Workqueue |
| [02-grpc-sync.md](02-grpc-sync.md) | gRPC 配置同步 | 调试 Controller↔Gateway 同步问题 |
| [03-data-plane.md](03-data-plane.md) | 基于 Pingora 的数据面 | 修改请求处理流程、理解代理生命周期 |
| [04-route-matching.md](04-route-matching.md) | 路由匹配引擎 | 修改路由逻辑、排查路由不匹配 |
| [05-plugin-system.md](05-plugin-system.md) | 插件系统 | 理解插件执行机制、修改插件框架 |
| [06-load-balancing.md](06-load-balancing.md) | 负载均衡 | 修改 LB 策略、理解后端选择 |
| [07-gateway-api.md](07-gateway-api.md) | Gateway API 支持 | 添加新的 Gateway API 资源支持 |
| [08-resource-system.md](08-resource-system.md) | 资源系统 | 添加新资源类型、理解 define_resources! |
| [09-core-layout.md](09-core-layout.md) | Core 分层定版 | 放置新模块、避免回到旧目录结构 |

## 架构总览图

```
                    ┌──────────────────────────────────────────────────────────┐
                    │                  edgion-controller                       │
                    │                                                          │
  YAML/K8s CRD ──► │  ConfCenter ──► Workqueue ──► ResourceProcessor          │
                    │  (File/K8s)     (per-kind)    (validate/preparse/parse)  │
                    │                                                          │
  edgion-ctl ────► │  Admin API (:5800)   ConfigSyncServer (gRPC :5810)       │
                    └─────────────────────────────┬────────────────────────────┘
                                                  │ gRPC Watch/List
                                                  ▼
                    ┌──────────────────────────────────────────────────────────┐
                    │                  edgion-gateway                          │
                    │                                                          │
                    │  ConfigSyncClient ──► ClientCache ──► Preparse           │
                    │                       (per-kind)                         │
                    │  Pingora Server                                          │
                    │  ├─ ConnectionFilter (TCP-level, StreamPlugins)          │
                    │  ├─ ProxyHttp (HTTP/gRPC lifecycle)                      │
                    │  │  ├─ request_filter     → route match + plugins        │
                    │  │  ├─ upstream_peer      → backend selection + LB       │
                    │  │  ├─ upstream_response  → response plugins             │
                    │  │  └─ logging            → AccessLog                    │
                    │  └─ TCP/UDP/TLS Routes                                   │
                    │                                                          │
                    │  Admin API (:5900)   Metrics API (:5901)                 │
                    └──────────────────────────────────────────────────────────┘
```
