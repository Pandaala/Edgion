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
| [00-overview.md](00-overview.md) | ConfigSync gRPC 协议总览 | 理解同步协议设计、Proto 定义、版本追踪 |
| [01-controller-side.md](01-controller-side.md) | Controller 侧 gRPC 服务端 | 调试 ConfigSyncServer、WatchObj、ClientRegistry |
| [02-gateway-side.md](02-gateway-side.md) | Gateway 侧 gRPC 客户端 | 调试 ConfigSyncClient、ClientCache、ConfHandler |

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

## 核心概念

- **ConfigSync 协议**：定义了 4 个 RPC 方法（GetServerInfo、List、Watch、WatchServerMeta），Gateway 通过这些方法与 Controller 同步资源配置。
- **版本追踪**：`sync_version`（单调递增）和 `server_id`（UUID）配合使用，实现增量同步和 Controller 重启检测。
- **不同步资源**：`ReferenceGrant` 和 `Secret` 是 `no_sync_kinds`，仅在 Controller 侧使用，不发送到 Gateway。
- **模块分布**：`common/conf_sync`（Proto + 共享 traits）、`controller/conf_sync`（服务端）、`gateway/conf_sync`（客户端）。
