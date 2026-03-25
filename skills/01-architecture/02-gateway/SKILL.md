---
name: 02-gateway
description: edgion-gateway 数据面架构：Pingora 集成、路由匹配、TLS 管理、插件系统、负载均衡、后端发现、可观测性、LinkSys。
---

# 02 Gateway 架构（edgion-gateway）

> edgion-gateway 是数据面，基于 Pingora 构建的高性能代理服务器。
> 从 Controller 通过 gRPC 接收配置，处理 HTTP/gRPC/TCP/TLS/UDP 流量。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-overview.md](00-overview.md) | Gateway 总体架构 | 首次了解 Gateway 设计 |
| [01-startup-shutdown.md](01-startup-shutdown.md) | 启动/关闭 + Pingora 集成 | 调试启动问题、理解初始化顺序 |
| [02-pingora-lifecycle.md](02-pingora-lifecycle.md) | ProxyHttp 回调、ConnectionFilter | 修改请求处理流程、理解代理生命周期 |
| [03-routes/](03-routes/) | 路由子系统 | 修改路由逻辑、排查路由不匹配 |
| [04-tls/](04-tls/) | TLS 子系统 | TLS 证书管理、mTLS 配置 |
| [05-plugin-system.md](05-plugin-system.md) | 插件系统（4 阶段执行模型） | 理解插件执行流程、开发新插件、修改条件执行逻辑 |
| [06-load-balancing.md](06-load-balancing.md) | 负载均衡策略 | 修改 LB 策略、理解 RoundRobin/EWMA/LeastConn/ConsistentHash 选择算法 |
| [07-backends.md](07-backends.md) | 后端发现 + 健康检查 | 修改 Endpoint/EndpointSlice 发现、健康检查配置、BackendTLSPolicy |
| [08-observe.md](08-observe.md) | 可观测性 | AccessLog 零拷贝输出、Prometheus 指标、协议日志 |
| [09-link-sys.md](09-link-sys.md) | LinkSys 外部系统集成 | ES/Redis/Etcd/Webhook/LocalFile 客户端管理、DataSender 实现 |
| [10-runtime-store.md](10-runtime-store.md) | 运行时存储 | Gateway/Listener 配置存储、ArcSwap 无锁读取模式 |
| [11-conf-handler-guidelines.md](11-conf-handler-guidelines.md) | ConfHandler 开发规范 | 实现/修改 ConfHandler 的 full_set/partial_update |
| [12-edgion-plugin-dev.md](12-edgion-plugin-dev.md) | HTTP 插件开发指南 | 开发新的 EdgionPlugin |
| [13-stream-plugin-dev.md](13-stream-plugin-dev.md) | Stream 插件开发指南 | 开发 TCP/TLS 层插件 |
| [14-link-sys-dev.md](14-link-sys-dev.md) | LinkSys 连接器开发指南 | 开发新的外部系统连接器 |

## 架构总览

```
                          edgion-gateway
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  ConfigSyncClient ──► ClientCache (per-kind) ──► Preparse    │
│                       ├── EventDispatch                      │
│                       └── ConfHandler (per-kind)             │
│                                                              │
│  Pingora Server                                              │
│  ├── ConnectionFilter (TCP 层, StreamPlugins)                │
│  │                                                           │
│  ├── ProxyHttp (HTTP/gRPC 生命周期)                          │
│  │   ├── early_request_filter → ACME, timeouts               │
│  │   ├── request_filter       → route match + plugins        │
│  │   ├── upstream_peer        → backend selection + LB       │
│  │   ├── upstream_response    → response plugins             │
│  │   └── logging              → AccessLog + Metrics          │
│  │                                                           │
│  ├── TCP/TLS Routes (per-port 路由)                          │
│  └── UDP Routes (per-port 路由)                              │
│                                                              │
│  Admin API (:5900)   Metrics API (:5901)                     │
│                                                              │
│  Backends                                                    │
│  ├── Service/EndpointSlice/Endpoint 发现                     │
│  ├── Health Check 管理                                       │
│  └── BackendTLSPolicy                                        │
│                                                              │
│  TLS 子系统                                                   │
│  ├── TLS Store (证书存储 + SNI 匹配)                          │
│  ├── 下游 TLS (客户端→网关)                                   │
│  └── 上游 TLS (网关→后端 mTLS)                               │
│                                                              │
│  LinkSys (ES, Redis, Etcd, Webhook, File)                    │
└──────────────────────────────────────────────────────────────┘
```
