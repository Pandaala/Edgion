---
name: link-overview
description: Controller↔Gateway gRPC 同步协议：Proto 定义、同步流程、版本追踪、资源变更事件。
---

# 同步协议总览

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/02-grpc-sync.md`

## 待填充内容

### Proto 定义

<!-- TODO: ConfigSync service 的 4 个 RPC 方法 -->
<!--
service ConfigSync {
    rpc GetServerInfo(ServerInfoRequest) returns (ServerInfoResponse);
    rpc List(ListRequest) returns (ListResponse);
    rpc Watch(WatchRequest) returns (stream WatchResponse);
    rpc WatchServerMeta(WatchServerMetaRequest) returns (stream ServerMetaEvent);
}
-->

### 同步流程

<!-- TODO:
Gateway 启动 → GetServerInfo → List(kind) for each kind → Watch(kind) for each kind
-->

### 版本追踪

<!-- TODO:
- sync_version: 单调递增计数器
- server_id: UUID，检测 Controller 重启/重载
- from_version: Gateway 续接版本
- expected_server_id: 检测过期数据
-->

### 资源变更事件

<!-- TODO:
ResourceChange enum:
- InitStart / InitAdd / InitDone
- EventAdd / EventUpdate / EventDelete
-->

### 模块分布

<!-- TODO:
- common/conf_sync: Proto 定义 + 共享 traits
- controller/conf_sync: 服务端实现
- gateway/conf_sync: 客户端实现
-->

### 不同步的资源

<!-- TODO: ReferenceGrant 和 Secret 不同步到 Gateway -->
