---
name: 05-resources
description: 资源分类与处理：通用处理流程、每种资源的功能点/特殊处理/跨资源关联。
---

# 05 资源架构

> 本目录描述每种资源从 Controller 到 Gateway 的**内部处理流程**：Handler 各阶段逻辑、requeue 关联、源码位置。
> 资源的**外部契约**（YAML Schema、字段类型与默认值）见 [02-features/03-resources/](../../02-features/03-resources/SKILL.md)——两边编号一一对齐，改代码通常两边都要看。
>
> 每篇资源文档遵循统一结构：
> 1. **通用流程引用** — 指向 [00-resource-flow.md](00-resource-flow.md) 的通用流程
> 2. **资源特有功能** — Handler 各阶段（filter/validate/preparse/parse/on_change/on_delete/update_status）的具体行为
> 3. **跨资源关联** — 该资源与其他资源之间的引用、级联更新、requeue 关系

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-resource-flow.md](00-resource-flow.md) | 资源通用处理流程 | 理解资源从 Controller 到 Gateway 的流转 |
| **核心配置** | | |
| [01-gateway.md](01-gateway.md) | Gateway 资源 | 修改 Gateway/Listener 逻辑 |
| [02-gateway-class.md](02-gateway-class.md) | GatewayClass 资源 | 修改 GatewayClass 处理 |
| [03-edgion-gateway-config.md](03-edgion-gateway-config.md) | EdgionGatewayConfig 资源 | 修改全局配置 |
| **路由** | | |
| [04-http-route.md](04-http-route.md) | HTTPRoute 资源 | 修改 HTTP 路由 |
| [05-grpc-route.md](05-grpc-route.md) | GRPCRoute 资源 | 修改 gRPC 路由 |
| [06-tcp-route.md](06-tcp-route.md) | TCPRoute 资源 | 修改 TCP 路由 |
| [07-tls-route.md](07-tls-route.md) | TLSRoute 资源 | 修改 TLS 路由 |
| [08-udp-route.md](08-udp-route.md) | UDPRoute 资源 | 修改 UDP 路由 |
| **安全与策略** | | |
| [09-edgion-tls.md](09-edgion-tls.md) | EdgionTls 资源 | 修改 TLS 证书管理 |
| [10-secret.md](10-secret.md) | Secret 资源 | 理解 Secret 处理和安全约束 |
| [11-reference-grant.md](11-reference-grant.md) | ReferenceGrant 资源 | 理解跨命名空间引用 |
| [12-backend-tls-policy.md](12-backend-tls-policy.md) | BackendTLSPolicy 资源 | 配置后端 TLS |
| **插件与扩展** | | |
| [13-edgion-plugins.md](13-edgion-plugins.md) | EdgionPlugins 资源 | 理解插件配置 |
| [14-edgion-stream-plugins.md](14-edgion-stream-plugins.md) | EdgionStreamPlugins 资源 | 理解 Stream 插件 |
| [15-plugin-metadata.md](15-plugin-metadata.md) | PluginMetaData 资源 | 理解插件元数据 |
| **后端与服务** | | |
| [16-service-endpoints.md](16-service-endpoints.md) | Service + EndpointSlice + Endpoint | 后端发现 |
| **ACME** | | |
| [17-edgion-acme.md](17-edgion-acme.md) | EdgionAcme 资源 | 自动证书 |
| **基础设施** | | |
| [18-link-sys.md](18-link-sys.md) | LinkSys 资源 | 外部系统连接 |

## 资源关联总图

```
GatewayClass ──────► Gateway ◄────── EdgionGatewayConfig
                       │
            ┌──────────┼──────────┐
            ▼          ▼          ▼
       HTTPRoute   GRPCRoute   TCP/TLS/UDPRoute
            │          │          │
            ▼          ▼          ▼
         Service ──► EndpointSlice/Endpoint
            │
            ▼
     BackendTLSPolicy

Gateway ──► Secret (TLS 证书)
         ──► EdgionTls (扩展 TLS)
         ──► EdgionAcme (自动证书)

HTTPRoute/GRPCRoute ──► EdgionPlugins (HTTP 插件)
                    ──► EdgionStreamPlugins (Stream 插件)
                    ──► PluginMetaData (插件元数据)

跨命名空间引用 ──► ReferenceGrant

AccessLog/Plugin 数据 ──► LinkSys (外部系统)
```
