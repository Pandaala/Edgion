---
name: gateway-backends
description: 后端发现与健康检查：Service/EndpointSlice/Endpoint 发现、健康检查管理、BackendTLSPolicy。
---

# 后端发现与健康检查

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### 后端发现

<!-- TODO:
backends/discovery/
├── endpoint/         # Kubernetes Endpoint 资源监听 + RoundRobin 存储
├── endpoint_slice/   # Kubernetes EndpointSlice 资源监听（推荐）
└── services/         # Kubernetes Service 资源监听
-->

### EndpointSlice vs Endpoint

<!-- TODO: K8s 1.21+ 推荐 EndpointSlice，两种 HA 支持模式 -->

### 健康检查

<!-- TODO:
backends/health/check/
├── 健康检查管理器
├── HTTP/TCP 探针执行
├── 配置存储
└── 状态存储
-->

### BackendTLSPolicy

<!-- TODO:
backends/policy/backend_tls/
- 上游 mTLS 配置
- 证书校验设置
-->

### 预加载

<!-- TODO: backends/preload.rs — 启动时 LB 预热 -->

### 端点验证

<!-- TODO: backends/validation/ — 路由中的端点验证 -->
