---
name: gateway-grpc-route-engine
description: gRPC 路由引擎：Service/Method 匹配、gRPC-Web 支持、HTTP 管线集成。
---

# gRPC 路由引擎

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### 模块结构

<!-- TODO:
routes/grpc/
├── conf_handler_impl.rs    # GRPCRoute 更新处理
├── match_engine/           # gRPC service/method 匹配
├── match_unit/             # 路由匹配信息结构
├── integration/            # HTTP 管线集成
└── routes_mgr/             # 路由管理器（per-domain）
-->

### Service/Method 匹配

<!-- TODO: gRPC 服务名 + 方法名的匹配规则 -->

### gRPC-Web 支持

<!-- TODO: gRPC-Web 协议适配 -->

### 与 HTTP 路由的关系

<!-- TODO: gRPC 路由复用 HTTP 的 per-port 隔离和 domain 匹配 -->
