---
name: controller-architecture
description: edgion-controller 控制面架构：总体设计、启动/关闭、Admin API、配置中心、Workqueue、ResourceProcessor、Requeue、CacheServer、ACME 服务。
---

# 01 Controller 架构（edgion-controller）

> edgion-controller 是控制面，负责资源的接收、校验、处理和分发。
> 从 K8s API 或本地文件系统接收资源配置，经过校验/预解析/处理后，通过 gRPC 同步给 Gateway 实例。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-overview.md](00-overview.md) | Controller 总体架构 | 首次了解 Controller 设计 |
| [01-startup-shutdown.md](01-startup-shutdown.md) | 程序启动与关闭流程 | 调试启动问题、理解初始化顺序 |
| [02-admin-api.md](02-admin-api.md) | Admin HTTP API (:5800) | 使用/扩展管理接口 |
| [03-config-center/](03-config-center/SKILL.md) | 配置中心子系统（ConfCenter trait + 双后端） | 修改资源处理流程、K8s HA |
| [04-workqueue.md](04-workqueue.md) | Workqueue 去重与退避机制 | 调试处理延迟、理解去重和重试逻辑 |
| [05-resource-processor.md](05-resource-processor.md) | ResourceProcessor 处理流水线 | 修改资源处理逻辑、理解 Handler 接口 |
| [06-requeue-mechanism.md](06-requeue-mechanism.md) | 跨资源 Requeue 机制 | 排查依赖联动、理解 TriggerChain |
| [07-cache-server.md](07-cache-server.md) | ServerCache + EventStore 内存缓存 | 调试 gRPC 同步数据源、理解版本机制 |
| [08-acme-service.md](08-acme-service.md) | ACME 证书自动化服务 | 修改/调试自动证书签发与续期 |

## 架构总览

```
                         edgion-controller
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  ConfCenter (FileSystem / Kubernetes)                       │
│  ├── ResourceController (per-kind watcher)                  │
│  │   └── Workqueue (dedup + backoff)                        │
│  │       └── ResourceProcessor                              │
│  │           ├── validate → preparse → parse → on_change    │
│  │           └── update ServerCache                         │
│  │                                                          │
│  ├── ProcessorRegistry (全局注册表)                          │
│  ├── SecretRefManager / ServiceRefManager                   │
│  ├── CrossNamespaceRefManager                               │
│  └── RequeueChain (跨资源联动)                               │
│                                                             │
│  Admin API (:5800)                                          │
│  ├── CRUD 接口 (namespaced / cluster)                       │
│  ├── health / ready                                         │
│  └── reload / server-info                                   │
│                                                             │
│  ConfigSyncServer (gRPC :50051)                             │
│  ├── List / Watch (per-kind)                                │
│  ├── WatchServerMeta                                        │
│  └── ClientRegistry (已连接 Gateway)                        │
│                                                             │
│  ACME Service (可选, 仅 Leader)                             │
│  └── Let's Encrypt 自动证书签发与续期                        │
└─────────────────────────────────────────────────────────────┘
```
