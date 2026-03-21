---
name: gateway-overview
description: edgion-gateway 总体架构：核心职责、模块划分、与 Controller 的关系。
---

# Gateway 总体架构

> **状态**: 框架已建立，待填充详细内容。

## 概要

edgion-gateway 是 Edgion 的数据面，基于 Pingora 构建，使用同步 Tokio + Pingora 主循环的双运行时模型。

## 待填充内容

### 核心职责

<!-- TODO:
- 从 Controller 接收配置（gRPC Watch/List）
- 管理 HTTP/gRPC/TCP/TLS/UDP 多协议代理
- 路由匹配与请求转发
- 插件执行（请求/响应两侧）
- 负载均衡与后端健康检查
- TLS 终止与 mTLS
- 可观测性（AccessLog + Metrics）
-->

### 模块划分

<!-- TODO: 对应 src/core/gateway/ 下的模块结构 -->
```
src/core/gateway/
├── api/           # Admin API (:5900)
├── backends/      # 后端发现、健康检查、BackendTLSPolicy
│   ├── discovery/ # Endpoint/EndpointSlice/Service
│   ├── health/    # 健康检查管理
│   └── policy/    # BackendTLSPolicy
├── cache/         # LRU 缓存
├── cli/           # 启动入口
├── conf_sync/     # 配置同步（gRPC 客户端）
├── config/        # GatewayClass/EdgionGatewayConfig 存储
├── lb/            # 负载均衡算法
├── link_sys/      # 外部系统集成
├── observe/       # 可观测性
├── plugins/       # 插件系统
│   ├── http/      # 28 个 HTTP 插件
│   ├── stream/    # Stream 插件
│   └── runtime/   # 插件执行引擎
├── routes/        # 多协议路由
│   ├── http/      # HTTP 路由匹配与代理
│   ├── grpc/      # gRPC 路由
│   ├── tcp/       # TCP 路由
│   ├── tls/       # TLS 路由
│   └── udp/       # UDP 路由
├── runtime/       # Pingora 运行时
│   ├── server/    # Listener 构建、错误响应
│   ├── matching/  # Gateway/TLS 匹配
│   └── store/     # 配置存储
├── services/      # 附加服务（ACME）
└── tls/           # TLS 证书管理
    ├── boringssl/  # BoringSSL 后端
    ├── openssl/    # OpenSSL 后端
    ├── runtime/    # TLS 回调
    ├── store/      # 证书存储
    └── validation/ # 证书校验
```

### 双运行时模型

<!-- TODO: Tokio 用于 async 任务（gRPC、配置处理），Pingora 主循环用于代理请求 -->
