# Gateway API 版本支持

Edgion 将 Gateway API 的“资源 API 版本”和“Gateway API bundle 版本”视为两个不同概念。本页说明当前可依赖的版本边界，以及不能据此推断的兼容性。

已测试的 Standard CRD 包版本及逐资源 YAML 覆盖情况，见
[Gateway API Standard CRD 兼容性](../ops-guide/gateway/gateway-api-compatibility.md)。

## 当前声明的资源版本

| 资源 | Edgion 声明的版本 | 说明 |
|------|-------------------|------|
| GatewayClass | `gateway.networking.k8s.io/v1` | Core |
| Gateway | `gateway.networking.k8s.io/v1` | Core |
| HTTPRoute | `gateway.networking.k8s.io/v1` | Core |
| GRPCRoute | `gateway.networking.k8s.io/v1` | Core |
| TLSRoute | `gateway.networking.k8s.io/v1` | Standard；仅支持 v1 |
| ReferenceGrant | `gateway.networking.k8s.io/v1` | Standard；Kubernetes 路径仍可探测 `v1beta1` |
| BackendTLSPolicy | `gateway.networking.k8s.io/v1` | 仅支持 v1 |
| TCPRoute | `gateway.networking.k8s.io/v1` | Standard |
| UDPRoute | `gateway.networking.k8s.io/v1` | Standard |

## v1.6.2 conformance 证据

在隔离的 kind 集群中安装 v1.6.2 Experimental CRD，并运行 Go conformance
v1.6.2：完整 `GATEWAY-HTTP` profile 的 Core 37/37、Extended 30/30 通过；
`GATEWAY-GRPC` 的 Core 15/15 通过。完整 `GATEWAY-TLS` profile 的 Core
20/20、Extended 2/3 通过。TLS 终止的单项测试通过，但在完整 profile 中超时，
因此 Edgion 暂不在 `GatewayClass.status.supportedFeatures` 中声明
`TLSRouteModeTerminate`。详见
[`HTTP/GRPC 报告`](../../../edgion-tests/conformance/reports/gateway-http-grpc-v1.6.2-kind-2026-09-23.yaml)
和 [`TLS 报告`](../../../edgion-tests/conformance/reports/gateway-tls-v1.6.2-kind-2026-09-23.yaml)。

TCPRoute/v1 和 UDPRoute/v1 在 Gateway 启动时已绑定的端口上能够接受路由并转发
实际流量。完整 v1.6.2 profile 在测试于启动后新建监听端口时超时；这些端口目前
需要重启 Gateway 才能绑定。因此 `TCPRoute` 和 `UDPRoute` 暂不列入
`GatewayClass.status.supportedFeatures`。完整 TLS profile 的超时仍未解决；
[监听端口生命周期](../../../skills/04-review/listeners/compatibility-constraints/listener-port-set-frozen-at-startup.md)
目前要求新增端口时重启 Gateway。
这些结果不表示完整 v1.6.2 conformance，也不表示任意已安装 CRD bundle 均兼容。

“集群 CRD 提供了某个版本”不等于“Edgion 声明支持该版本”。完整支持还需要相应的读取方式、类型转换、语义处理、状态更新和测试。

## Bundle 与 API 版本

仓库当前携带 Gateway API v1.6.2 CRD bundle。Go 模块依赖 `sigs.k8s.io/gateway-api` v1.6.2 和 `sigs.k8s.io/gateway-api/conformance` v1.6.2；这些版本号描述不同的构件，不能合并成一个支持声明。

离线检查覆盖 v1.0.0 至 v1.6.2 之间实际发布的 13 个稳定版本，分别检查 Standard 和 Experimental 两种 bundle：266 条最小 OpenAPI fixture 通过，72 条因对应 bundle 未提供该源 GVK 而标记为 N/A。具体版本、下载地址、SHA-256 和字段路径见 `edgion-tests/gateway_api_bundles/`。最小 fixture 通过不代表完整字段、CEL 或运行时兼容。

同一个 GVK，例如 `gateway.networking.k8s.io/v1/HTTPRoute`，在不同 bundle 中可能具有略有差异的 schema。Standard channel 要求兼容演进；Experimental channel 不提供相同保证。

## HTTPRoute v1 内部的功能稳定级别

`v1` 标识约束的是 HTTPRoute API 表示，并不表示其中每个可选功能都已达到稳定状态。在仓库携带的 Gateway API v1.6.2 CRD 中，Experimental channel 会在相同的 `HTTPRoute/v1` GVK 内增加 `spec.useDefaultGateways`、`spec.rules[].retry`、`spec.rules[].sessionPersistence`，以及规则和后端过滤器中的 `externalAuth`。

Edgion 已建模并在运行时处理 retry 和 session persistence，但目前未将它们声明为 Gateway API conformance feature。`useDefaultGateways` 会保留在模型中，以便拒绝 `All`，避免静默忽略。上游 ExternalAuth 的 `backendRef`／`protocol` 形式也会保留，以便拒绝受影响的规则；Edgion 自有的 ForwardAuth 扩展使用不同的配置形式。因此，资源版本支持、schema 接受、运行时实现和 `GatewayClass.status.supportedFeatures` 是四类独立声明。

这些字段不应被描述成另一个 HTTPRoute alpha API 版本；它们是 v1 表示内部的 Experimental 功能。Extended 也不一定等于 Experimental channel 专属，例如仓库中的 CORS 字段同时存在于两个 channel。

## Kubernetes 模式

Kubernetes API Server 根据集群实际安装的 CRD 执行 schema、默认值、字段裁剪和 CEL 校验。Edgion Controller 随后读取选定 API 版本的对象并执行实现层语义校验。

当前启动检查只确认所需 GVK 是否由集群提供，尚未根据 CRD 的 `gateway.networking.k8s.io/bundle-version` 和 channel 注解建立完整兼容性结论。因此，不应将任意更新后的 Gateway API CRD 视为已经通过 Edgion 验证。

`GatewayClass.status.conditions[SupportedVersion]` 因此为 `Unknown`，reason 为 `CompatibilityUnknown`，不表示已验证当前集群安装的 bundle。Gateway/v1 自 v1.3.0 起允许省略 `spec.addresses[].value`；Edgion 会读取该对象，标记 `Accepted=False` 并撤销其 listener，不会自行分配地址。BackendTLSPolicy/v1 自 v1.5.0 起允许非 `System` 的 `wellKnownCACertificates` 名称；Edgion 会读取该对象、报告不支持并拒绝发布策略。同一 List 中的其他对象仍可处理。

Gateway v1.6.2 的 `spec.tls.frontend` 最多允许 16 个 CA 引用。Edgion 的解码、schema、Controller 解析和 Gateway TLS 编译均使用这一上限；CA 材料的独立总量限制仍然生效。

## 非 Kubernetes 模式

Gateway API 文档必须明确声明已登记源版本对应的 `apiVersion` 和 `kind`；文件系统、etcd 和 Admin 输入会拒绝缺失、格式错误或未登记的 TypeMeta。TLSRoute 仅支持 v1；加载旧清单前请将其更新为 v1。GatewayClass、Gateway、HTTPRoute 和 ReferenceGrant 的 v1beta1 来源有显式转换适配器，但尚缺逐来源 conformance 证据。Gateway conf-sync 仅接受 Controller 输出的 canonical TypeMeta。

文件系统、etcd 和 Admin API 没有 Kubernetes API Server 负责版本转换。接受 canonical 版本之外的输入版本必须有对应 schema、源类型和显式转换；仅仅忽略 `apiVersion` 后反序列化成功，不构成版本支持。

## 兼容性承诺

- 当前配置应使用表中声明的版本。
- 不要依赖 CRD 中额外 served 版本，除非 Edgion 文档明确声明并有测试覆盖。
- Standard channel 的添加的未知字段可能被旧版 Edgion 忽略；被忽略的字段不会产生运行时功能。
- Experimental 资源和字段可能在 Gateway API bundle 升级时发生不兼容变化。
- 即使资源 GVK 已受支持，也必须单独核对字段级功能支持。
- Gateway API CRD 与 Edgion 的升级应作为一次协调变更执行。

## 相关资料

- [Gateway API 版本策略](https://gateway-api.sigs.k8s.io/docs/concepts/versioning/)
- [Gateway API CRD 管理](https://gateway-api.sigs.k8s.io/guides/crd-management/)
- [资源架构总览](./resource-architecture-overview.md)
