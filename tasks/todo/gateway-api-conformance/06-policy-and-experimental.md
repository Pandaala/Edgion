# 前瞻性功能与实验性特性

优先级: P2

## 1. BackendTrafficPolicy

### 规范描述

Gateway API v1.4 引入实验性 BackendTrafficPolicy，用于在策略附着层级配置：
- 连接池管理（max connections、idle timeout）
- 熔断器（circuit breaking）
- 健康检查策略
- 重试策略（作为 policy 而非内嵌 route 字段）

### 当前状态

Edgion 无此资源定义。相关功能分散在：
- `health_check.rs` — 健康检查配置
- `edgion_gateway_config.rs` — 全局连接配置
- `tasks/todo/circuit-breaker/` — 熔断器已在计划中

### 建议

- 先完成 circuit-breaker 任务
- 后续考虑将连接池、熔断、健康检查统一到 BackendTrafficPolicy 模型

---

## 2. externalAuth 原生过滤器 (GEP-1494)

### 规范描述

Gateway API 实验性 `type: ExternalAuth` 过滤器，标准化外部认证机制。

### 当前状态

- Edgion 有 `ForwardAuth` 插件，功能等价
- 但仅通过 `ExtensionRef` 使用，非一等过滤器类型
- 用户需要知道 Edgion 特有的插件名称

### 建议

- 在 `HTTPRouteFilterType` 枚举中添加 `ExternalAuth` 变体
- 内部映射到现有 `ForwardAuth` 实现
- 保持 ExtensionRef 方式的向后兼容

---

## 3. Gateway infrastructure 字段

### 规范描述

Extended 功能，允许用户请求基础设施资源（如 IP 地址、负载均衡器类型等）。

### 当前状态

`GatewaySpec` 无 `infrastructure` 字段。

### 建议

低优先级。仅在有 cloud provider 集成需求时考虑。

---

## 4. frontendValidation 标准化

### 现状

- mTLS 客户端认证通过 Edgion 私有 `EdgionTls.client_auth` 实现
- 功能完整：CA ref、验证深度、SAN/CN 白名单
- 但不遵循 Gateway API 标准 `listener.tls.frontendValidation` 字段

### 建议

- 添加 `frontendValidation` 字段到 Gateway listener TLS 配置
- 与 EdgionTls 的 client_auth 配置做合并/映射
- EdgionTls 保留作为更丰富的扩展方式

---

## 5. ConformanceProfiles 声明

### 现状

- conformance test 使用 `SupportedFeatures` 集合但未声明 ConformanceProfiles
- v1.1+ conformance 框架支持基于 profile 的报告

### 建议

- 在 conformance test 中声明 `HTTPConformanceProfile`、`GRPCConformanceProfile` 等
- 生成标准化的 conformance 报告
