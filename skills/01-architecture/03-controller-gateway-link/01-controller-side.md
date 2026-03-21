---
name: link-controller-side
description: Controller 侧 gRPC 实现：ConfigSyncServer、WatchObj trait、ClientRegistry、事件流。
---

# Controller 侧实现

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### ConfigSyncServer

<!-- TODO:
- 持有所有 WatchObj（来自各 ResourceProcessor 注册）
- server_id: 每次启动唯一
- list(): 返回某种资源的全量
- watch(): 流式推送某种资源的变更
-->

### WatchObj trait

<!-- TODO:
- 对象安全的 list/watch 接口
- list(): 获取所有资源 + 快照版本
- watch_stream(): 通过 mpsc channel 流式推送事件
-->

### gRPC 服务实现

<!-- TODO:
- ConfigSyncGrpcServer: 实现 ConfigSync proto service
- 路由 List/Watch 到对应 processor 的 cache
- 支持 resourceVersion 续接
-->

### ClientRegistry

<!-- TODO:
- 追踪已连接的 Gateway 实例
- 用于集群级限流
- 维护 Gateway 身份和状态
-->

### 模块结构

<!-- TODO:
controller/conf_sync/
├── conf_server/
│   ├── config_sync_server.rs
│   ├── grpc_server.rs
│   ├── client_registry.rs
│   └── traits.rs
└── cache_server/
    ├── cache.rs
    ├── store.rs
    └── types.rs
-->
