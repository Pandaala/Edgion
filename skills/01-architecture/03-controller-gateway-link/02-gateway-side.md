---
name: link-gateway-side
description: Gateway 侧 gRPC 实现：ConfigSyncClient、ClientCache、EventDispatch、ConfHandler 分发。
---

# Gateway 侧实现

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### ConfigSyncClient

<!-- TODO:
- 连接 Controller gRPC 服务
- 管理多种资源的 Watch 连接
- 断线重连机制
-->

### ClientCache

<!-- TODO:
- 内存缓存，per-kind 存储
- 接收 gRPC 事件并更新本地状态
-->

### EventDispatch

<!-- TODO:
- 将资源变更事件分发到各 ConfHandler
- 事件类型：InitStart/InitAdd/InitDone/EventAdd/EventUpdate/EventDelete
-->

### ConfHandler 机制

<!-- TODO:
- 每种资源有对应的 ConfHandler
- 处理资源变更，更新 Gateway 运行时状态
- 如路由重建、证书更新、LB 重配置等
-->

### server_id 变化检测

<!-- TODO:
- 监控 Controller 的 server_id
- 变化时触发全量 relist
- 确保配置一致性
-->

### 模块结构

<!-- TODO:
gateway/conf_sync/
├── conf_client/
│   ├── config_client.rs
│   ├── grpc_client.rs
│   └── mod.rs
└── cache_client/
    ├── cache.rs
    ├── cache_data.rs
    ├── event_dispatch.rs
    └── mod.rs
-->
