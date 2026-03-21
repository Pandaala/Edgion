---
name: controller-gateway-link
description: Controller↔Gateway 双向 gRPC 同步架构：ConfigSync 协议、Watch/List 机制、两侧实现。
---

# 03 Controller↔Gateway Link

> Controller 和 Gateway 之间通过 gRPC 双向通信实现配置同步。
> Gateway 作为客户端连接 Controller，通过 List 获取全量、Watch 获取增量。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-overview.md](00-overview.md) | 整体架构 + Proto 定义 | 理解同步协议设计 |
| [01-controller-side.md](01-controller-side.md) | Controller 侧实现 | 调试 gRPC 服务端 |
| [02-gateway-side.md](02-gateway-side.md) | Gateway 侧实现 | 调试 gRPC 客户端 |

## 架构总览

```
  Controller                                          Gateway
┌─────────────────────────┐                ┌─────────────────────────┐
│                         │                │                         │
│  ResourceProcessor      │                │  ConfigSyncClient       │
│  └── ServerCache<T>     │   gRPC         │  ├── grpc_client        │
│      └── EventStore     │◄──────────────►│  └── ConfigClient       │
│                         │                │      └── ClientCache    │
│  ConfigSyncServer       │   List/Watch   │         └── EventDispatch
│  ├── WatchObj (per-kind)│──────────────►│             └── ConfHandler
│  └── ClientRegistry    │                │                         │
│                         │   WatchMeta    │                         │
│  server_id (per-start)  │──────────────►│  检测 server_id 变化    │
│                         │                │  → 触发全量 relist      │
└─────────────────────────┘                └─────────────────────────┘
```
