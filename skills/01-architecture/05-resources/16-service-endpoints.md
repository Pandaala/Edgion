---
name: resource-service-endpoints
description: Service + EndpointSlice + Endpoint 资源：后端发现、端点解析、两种 HA 模式。
---

# Service / EndpointSlice / Endpoint 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 概要

这三种资源共同实现后端服务发现，将路由的 backendRefs 解析为实际的后端端点。

## 待填充内容

### Service

<!-- TODO:
- Kubernetes Service 资源
- 路由的 backendRefs 引用目标
- ServiceRefManager 追踪依赖
- Service 变更时 requeue 所有引用它的路由
-->

### EndpointSlice（推荐）

<!-- TODO:
- K8s 1.21+ 推荐的端点发现方式
- 更好的扩展性（分片）
- EndpointSliceHandler
-->

### Endpoint（旧版）

<!-- TODO:
- 传统端点发现方式
- EndpointsHandler
- 与 EndpointSlice 的选择逻辑
-->

### 两种 HA 支持模式

<!-- TODO: 参见 VersionDetection，自动检测 K8s API 能力 -->

### Controller 侧处理

<!-- TODO: ServiceHandler, EndpointSliceHandler, EndpointsHandler -->

### Gateway 侧处理

<!-- TODO:
- 后端发现和更新
- RoundRobin 存储
- 健康检查状态
-->

### 跨资源关联

<!-- TODO:
- ← HTTPRoute/GRPCRoute/TCPRoute/TLSRoute/UDPRoute: backendRefs
- → BackendTLSPolicy: 后端 TLS 策略
-->
