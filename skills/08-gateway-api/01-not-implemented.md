---
name: gateway-api-not-implemented
description: Gateway API 规范中 Edgion 明确不实现的功能清单及原因
---

# Gateway API — 不实现的功能

本文件记录 Gateway API 规范中 Edgion 经过评估后决定不实现的功能。

## Programmed / Ready 状态条件

**决定：不实现**

Gateway API 规范将 `Programmed` 列为 Core 必需条件，要求确认配置已编程到数据面。Edgion 的 Controller-Gateway 架构使用异步 gRPC 配置同步（Gateway 主动 Watch/List 拉取），Controller 无法得知 Gateway 是否已加载某条具体配置。

曾评估过 Controller → Gateway HTTP Pull 方案（详见 `tasks/todo/data-plane-status-feedback.md`，设计已完成到 Step 03），但引入了全新通信路径、proto 改动、延时队列等，复杂度过高。轻量方案（推送成功即标记）也不成立，因为当前是 Gateway 拉取而非 Controller 推送。

- Edgion 仅设置 `Accepted` 和 `ResolvedRefs` 条件
- 详细架构决策见 [skills/01-architecture/01-controller/10-status-management.md](../01-architecture/01-controller/10-status-management.md)
- 完整方案设计见 [tasks/todo/data-plane-status-feedback.md](../../tasks/todo/data-plane-status-feedback.md)（保留备查，不实施）

## Gateway addresses 实际绑定地址上报

**决定：不实现**

Gateway API 规范期望 `Gateway.status.addresses` 反映数据面实际绑定的地址。同样受限于无数据面→Controller 状态回传通道。

当前行为：从 `spec.addresses`、Service ClusterIP、默认配置、fallback `0.0.0.0` 推导，不反映运行时实际状态。

- 相关合规任务见 [tasks/todo/gateway-api-conformance/03-status-conditions.md](../../tasks/todo/gateway-api-conformance/03-status-conditions.md)

## GRPCRoute 与 HTTPRoute 跨类型 Hostname 冲突检测

**决定：不实现**

Gateway API 规范建议同一 Listener 上 GRPCRoute 和 HTTPRoute 的 hostname 具有唯一性，冲突时应设置 status condition。

Edgion 当前架构中 gRPC 请求先匹配 GRPCRoute、不匹配再 fallback HTTPRoute，优先级逻辑合理。同一 hostname 上 GRPCRoute + HTTPRoute 共存是正常使用模式，主流实现（Envoy Gateway、Istio）也不做跨类型冲突检测。

- 相关任务见 [tasks/todo/gateway-api-conformance/04-grpcroute-gaps.md](../../tasks/todo/gateway-api-conformance/04-grpcroute-gaps.md)

## Gateway infrastructure 字段

**决定：暂不实现**

Gateway API Extended 功能，允许用户在 `GatewaySpec.infrastructure` 中声明期望的基础设施资源（IP 地址、负载均衡器类型、云厂商参数等）。Gateway Controller 据此自动创建/配置云资源。

Edgion 是自部署网关，不负责创建云基础设施资源，当前无 cloud provider 集成需求。

- 仅在有云厂商集成需求时考虑实现
- 相关任务见 [tasks/todo/gateway-api-conformance/06-policy-and-experimental.md](../../tasks/todo/gateway-api-conformance/06-policy-and-experimental.md)

## BackendTrafficPolicy (BTP)

**决定：暂不实现（等 GA）**

Gateway API v1.4 引入实验性 BackendTrafficPolicy CRD，统一管理连接池、熔断器、健康检查、重试策略。

Edgion 已通过 annotation 分散实现了这些功能（`edgion.io/circuit-breaker`、`health_check.rs`、`edgion_gateway_config.rs`），功能完整，但不通过标准 BTP CRD 暴露。

- 等 BTP 从实验性毕业到 GA 后再考虑统一到标准 CRD
- 当前 annotation 方式对用户可用，不影响实际功能
- 相关任务见 [tasks/todo/gateway-api-conformance/06-policy-and-experimental.md](../../tasks/todo/gateway-api-conformance/06-policy-and-experimental.md)

## ~~frontendValidation 标准化~~ ✅ 已完成

`GatewayTLSConfig` 已添加 `frontend_validation` 字段，Controller 解析 CA Secret 引用，Gateway TLS 握手时配置 mTLS verify mode。与 EdgionTls `client_auth` 完全独立，两条路径互不映射。`supportedFeatures` 已声明 `GatewayFrontendClientCertificateValidation`。

## GATEWAY-TLS ConformanceProfile

**决定：暂不声明**

TLS Passthrough 功能已实现，但 TLSRoute conformance tests 从未实际运行过。预计需要修复 status condition 相关逻辑（ResolvedRefs、Accepted、supportedKinds for TLSRoute）才能通过 core tests。

- 需在 SupportedFeatures 中声明 `SupportTLSRoute` 并跑通 8 个 core tests 后再声明 profile
- Extended tests（Terminate/Mixed 模式）可后续再做
- MESH-HTTP / MESH-GRPC：Edgion 不是 service mesh，不适用
- 相关任务见 [tasks/todo/gateway-api-conformance/06-policy-and-experimental.md](../../tasks/todo/gateway-api-conformance/06-policy-and-experimental.md)
