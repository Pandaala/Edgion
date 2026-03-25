# 前瞻性功能与实验性特性

优先级: P2

---

## 1. BackendTrafficPolicy

### 规范描述

Gateway API v1.4 引入实验性 BackendTrafficPolicy CRD，在策略附着层级配置：
- 连接池管理（max connections、idle timeout）
- 熔断器（circuit breaking）
- 健康检查策略
- 重试策略（作为 policy 而非内嵌 route 字段）

### 当前状态

- **无 BackendTrafficPolicy CRD 或类型定义**
- 熔断器已通过 Service annotation `edgion.io/circuit-breaker` 完整实现（`src/core/gateway/backends/circuit_breaker/`），包含弹出逻辑、LB 过滤、Prometheus 指标、集成测试
- 健康检查通过 `health_check.rs` 配置
- 全局连接配置在 `edgion_gateway_config.rs`
- 这些功能分散实现，未统一到标准 BTP 模型

### 差距

Edgion 的功能实质已部分覆盖 BTP 的范围，但通过 annotation 而非标准 CRD 暴露，不符合 Gateway API 标准路径。

### 建议

- 低优先级：等 BTP 从实验性毕业到 GA 后再考虑统一
- 当前 annotation 方式对用户可用，功能完整

---

## ~~2. ExternalAuth 原生过滤器 (GEP-1494)~~ ✅ 已完成

- `HTTPRouteFilterType` 添加 `ExternalAuth` 变体
- `HTTPRouteFilter` 添加 `external_auth: Option<ForwardAuthConfig>` 字段
- `plugin_runtime.rs` 中 `ExternalAuth` 分支映射到 `ForwardAuth::new`
- ExtensionRef 方式保留向后兼容

---

## 3. Gateway infrastructure 字段

### 规范描述

Extended 功能，允许用户请求基础设施资源（IP 地址、负载均衡器类型等）。

### 当前状态

- `GatewaySpec`（`src/types/resources/gateway.rs:29-40`）仅有 `gateway_class_name`、`listeners`、`addresses`，无 `infrastructure` 字段

### 建议

低优先级。仅在有 cloud provider 集成需求时考虑。

---

## ~~4. frontendValidation 标准化~~ ✅ 已完成

### 实现方案

- `GatewayTLSConfig` 新增 `frontend_validation` 字段（`FrontendTLSValidation` 结构体，含 `ca_certificate_refs`）
- Controller 侧：`parse()` 解析 CA Secret 引用并填充 `resolved_frontend_ca_secrets`；`validate_listener_resolved_refs()` 校验 CA cert refs（kind、group、Secret 存在性、`ca.crt` key、跨命名空间 ReferenceGrant）
- Gateway 侧：`GatewayTlsEntry` 携带 frontendValidation 数据；TLS 握手时 `configure_gateway_frontend_mtls()` 创建 X509Store 并设置 `PEER | FAIL_IF_NO_PEER_CERT` verify mode
- `handshake_complete_callback` 中增加 `is_mtls` 条件提取客户端证书信息
- `supportedFeatures` 声明 `GatewayFrontendClientCertificateValidation`
- 与 EdgionTls `client_auth`（SAN/CN 白名单、verify depth 等）完全独立，两条路径互不映射

---

## 5. ConformanceProfiles 声明

### 规范描述

v1.1+ conformance 框架支持基于 profile 的报告，声明 `HTTPConformanceProfile`、`GRPCConformanceProfile` 等。

### 当前状态

- ✅ 已实现：`conformance_test.go` 声明 `GATEWAY-HTTP` ConformanceProfile
- ⚠️ 已声明：`GATEWAY-GRPC` profile 已加入 conformance_test.go 和 run.sh，但未实际跑过 conformance test。GRPCRoute 功能（exact/header/hostname/weight 匹配）已实现，可能需要修 status condition 才能全部通过。
- ✅ 已实现：`run.sh` 支持 `--conformance-profiles` 参数，默认 `GATEWAY-HTTP,GATEWAY-GRPC`
- ✅ 报告生成已由 `-report-output` 和 `-conformance-profiles` 联动支持
- 待实现：GATEWAY-TLS profile — TLS Passthrough 已实现，但 conformance tests 从未跑过，需加 `SupportTLSRoute` 并修 status conditions（ResolvedRefs、supportedKinds）后再声明
