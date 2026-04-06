---
name: gateway-api-not-implemented
description: Gateway API 规范中 Edgion 明确不实现的功能清单及原因
---

# Gateway API — 不实现的功能

本文件记录 Gateway API 规范中 Edgion 经过评估后决定不实现的功能。

## Programmed / Ready 状态条件

**决定：简化实现（已完成）**

Gateway API 规范将 `Programmed` 列为 Core 必需条件，要求确认配置已编程到数据面。Edgion 的 Controller-Gateway 架构使用异步 gRPC 配置同步（Gateway 主动 Watch/List 拉取），Controller 无法精确得知 Gateway 是否已加载某条具体配置。

曾评估过 Controller → Gateway HTTP Pull 方案（详见 `tasks/todo/data-plane-status-feedback.md`，设计已完成到 Step 03），但引入了全新通信路径、proto 改动、延时队列等，复杂度过高。

当前采用简化策略：Controller 处理资源后即设置 `Programmed=True`（因为配置会通过 gRPC 同步推送到数据面），当 listener 存在冲突、未解析引用、无效 TLS 配置等异常时设置 `Programmed=False`。这满足 conformance test 的要求，但不代表数据面已实际加载配置。

- Gateway 级和 Listener 级均设置 `Programmed` 条件
- 详细架构决策见 [skills/01-architecture/01-controller/10-status-management.md](../01-architecture/01-controller/10-status-management.md)
- 完整方案设计见 [tasks/todo/data-plane-status-feedback.md](../../tasks/todo/data-plane-status-feedback.md)（保留备查，不实施）

## Gateway addresses 实际绑定地址上报 / GatewayStaticAddresses / GatewayOptionalAddressValue

**决定：不实现**

`GatewayStaticAddresses` conformance test 要求 Gateway 实际绑定到 `spec.addresses` 指定的地址。Edgion 是预部署的 binary，监听地址来自 TOML 配置，不会根据 Gateway `spec.addresses` 动态改变绑定行为。同样受限于无数据面→Controller 状态回传通道。

当前行为：`derive_gateway_addresses()` 将 `spec.addresses` 复制到 `status.addresses`，但数据面实际绑定地址不变（仍由 TOML 配置决定）。不声明 `SupportGatewayStaticAddresses` 和 `SupportGatewayAddressEmpty`。

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

## ListenerSet

**决定：暂不实现**

Gateway API v1.5 引入 `ListenerSet`，允许在不直接修改 `Gateway` 的情况下，
由其他资源向现有 Gateway 追加 listeners，并通过 `allowedListeners`、
命名空间约束和 `ReferenceGrant` 控制授权关系。

Edgion 当前不计划实现该能力，原因：

- Edgion 更偏向单团队或单控制面直接管理 `Gateway` 的使用模型，独立 `Gateway` 比共享 `Gateway` + 委托追加 listener 更自然
- `ListenerSet` 会显著增加资源模型复杂度：listener 合并、优先级、冲突检测、route parentRef 语义、attachedRoutes 统计、status 回写都需要扩展
- 该能力主要解决多团队共享单个 Gateway 的治理问题，不是 Edgion 当前的核心代理能力缺口

因此：

- 不声明 `SupportListenerSet`
- 不以 ListenerSet conformance tests 作为当前目标
- 若未来出现明确的多团队共享 Gateway 诉求，再重新评估

## HTTPRoute Multiple Mirrors

**决定：不实现**

Gateway API 的 `HTTPRouteRequestMultipleMirrors` 允许单条 HTTPRoute 规则同时配置多个 request mirror 目标。

Edgion 当前保留单 mirror 能力，但不计划扩展到多 mirror，原因：

- 多 mirror 属于较弱需求，收益不足以覆盖实现和维护成本
- 会增加请求复制、body tee、异步任务管理、日志与指标聚合的复杂度
- 对当前 Edgion 目标场景，单 mirror 已足够覆盖大多数 shadow/审计类用法

因此：

- 不声明 `SupportHTTPRouteRequestMultipleMirrors`
- 不以 `HTTPRouteRequestMultipleMirrors` conformance test 作为当前目标
- 保留现有单 mirror 能力，不向多目标镜像扩展

## TLSRouteModeMixed

**决定：不实现**

Edgion 支持：

- `TLSRoute` + `TLS listener(mode=Passthrough)`
- `TLSRoute` + `TLS listener(mode=Terminate)`

但明确不支持 `TLSRouteModeMixed`，即：

- 在同端口、同 listener 能力域内，按 SNI 动态决定一条 TLS 连接是 passthrough 还是 terminate

原因：

- 需要单个 TCP listener 先读取 `ClientHello` 再动态切换处理路径，架构复杂度明显上升
- 同一入口混用 passthrough / terminate 会增加证书选择、流量走向和安全边界的排障复杂度
- 对当前 Edgion 目标场景，标准 `Passthrough` 和标准 `Terminate` 已足够覆盖主要需求

因此：

- `TLSRouteModeTerminate` 可以作为后续 conformance 目标推进
- `TLSRouteModeMixed` 不作为当前实现目标，也不应在 supported features 中声明

## GatewayPort8080 / HTTPRouteParentRefPort / 动态端口绑定

**决定：不实现**

`SupportGatewayPort8080` 和 `SupportHTTPRouteParentRefPort` conformance features 都要求网关能动态绑定非标准端口（8080、8090 等）。

Edgion 的 Gateway 进程在**启动时**通过 `configure_listeners` 绑定所有端口。端口绑定是一次性操作：Pingora 服务初始化后无法在运行时动态添加新端口。如果某个 Gateway 资源在网关进程启动后才被创建（如 conformance 测试的临时 Gateway），其监听端口无法被绑定。

- 不声明 `SupportGatewayPort8080`（已从 `conformance_test.go` 和 `run.sh` 移除）
- 不声明 `SupportHTTPRouteParentRefPort`（已从 `conformance_test.go` 注释移除，2026-03-27）
- conformance 报告中 `GatewayWithAttachedRoutesWithPort8080`、`GatewayPort8080`、`HTTPRouteListenerPortMatching`、`HTTPRouteRedirectPortAndScheme` 标记为 skipped
- 若未来需要支持动态端口，需要重新评估 Pingora 的 `Server::add_service` 是否支持运行时调用，以及 Controller→Gateway 配置同步的时序问题

## BackendTLSPolicy Conformance

**决定：已实现（声明 `SupportBackendTLSPolicy`，暂不声明 `SupportBackendTLSPolicySANValidation`）**

数据面和控制面 status 均已实现：
- ancestorRef 正确指向关联的 Gateway（通过 Service → Route → Gateway 三步查询）
- 支持 Secret 和 ConfigMap 类型的 `caCertificateRefs`
- 同一 Service 多 Policy 冲突检测（按 creationTimestamp + name 排序）
- Status reason 值符合 conformance 精确要求（`InvalidKind`、`InvalidCACertificateRef`、`Conflicted`）
- Route on_change/on_delete 自动 requeue 关联的 BTP 以更新 ancestor 状态

Status 报告的局限性：`ancestorRef` 指向 Gateway 表示"控制面认为该 Policy 在此 Gateway 上生效"，但不代表数据面 TLS 连接一定成功（证书不匹配、后端不支持 TLS 等运行时问题无法通过 status 反映）。

**适用范围：仅 HTTPRoute / GRPCRoute。** BackendTLSPolicy 不对 TLSRoute 生效。TLSRoute Passthrough 透传原始 TLS 流量，Gateway 不与后端建立新连接；TLSRoute Terminate 按 SNI 转发解密流量到后端，属于纯 stream 层代理，不涉及 L7 后端 TLS 配置。需要后端 TLS（re-encrypt）的场景应使用 HTTPRoute / GRPCRoute + BackendTLSPolicy。

## BackendTrafficPolicy (BTP)

**决定：暂不实现（等 GA）**

Gateway API v1.4 引入实验性 BackendTrafficPolicy CRD，统一管理连接池、熔断器、健康检查、重试策略。

Edgion 已通过 annotation 分散实现了这些功能（`edgion.io/circuit-breaker`、`health_check.rs`、`edgion_gateway_config.rs`），功能完整，但不通过标准 BTP CRD 暴露。

- 等 BTP 从实验性毕业到 GA 后再考虑统一到标准 CRD
- 当前 annotation 方式对用户可用，不影响实际功能
- 相关任务见 [tasks/todo/gateway-api-conformance/06-policy-and-experimental.md](../../tasks/todo/gateway-api-conformance/06-policy-and-experimental.md)

## ~~frontendValidation 标准化~~ ✅ 已完成

Controller 解析 `frontend_validation` CA 引用（Secret/ConfigMap），per-listener 合并，设置 listener status conditions。Gateway 数据面 `configure_gateway_frontend_mtls()` 加载 CA 证书配置 SSL verify mode：AllowValidOnly 用 `PEER | FAIL_IF_NO_PEER_CERT`，AllowInsecureFallback 用自定义 verify callback 始终 accept（boringssl FFI）。与 EdgionTls `client_auth` 完全独立。

## GATEWAY-TLS ConformanceProfile

**决定：暂不声明**

TLS Passthrough 功能已实现；TLS Terminate 也已有数据面主路径，但 TLSRoute conformance tests 尚未系统跑通。预计需要修复 status condition 相关逻辑（ResolvedRefs、Accepted、supportedKinds for TLSRoute）并补 terminate 测试后，才能声明 profile。

- 需在 SupportedFeatures 中声明 `SupportTLSRoute` 并跑通 core passthrough tests 后再声明 profile
- `TLSRouteModeTerminate` 可作为后续增强继续推进
- `TLSRouteModeMixed` 明确不实现
- MESH-HTTP / MESH-GRPC：Edgion 不是 service mesh，不适用
- 相关任务见 [tasks/todo/gateway-api-conformance/06-policy-and-experimental.md](../../tasks/todo/gateway-api-conformance/06-policy-and-experimental.md)
