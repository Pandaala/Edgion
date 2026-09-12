# Gateway API 版本支持

Edgion 将 Gateway API 的“资源 API 版本”和“Gateway API bundle 版本”视为两个不同概念。本页说明当前可依赖的版本边界，以及不能据此推断的兼容性。

## 当前声明的资源版本

| 资源 | Edgion 声明的版本 | 说明 |
|------|-------------------|------|
| GatewayClass | `gateway.networking.k8s.io/v1` | Core |
| Gateway | `gateway.networking.k8s.io/v1` | Core |
| HTTPRoute | `gateway.networking.k8s.io/v1` | Core |
| GRPCRoute | `gateway.networking.k8s.io/v1` | Core |
| TLSRoute | `gateway.networking.k8s.io/v1` | Standard；代码仍有待清理的 `v1alpha3` 兼容路径 |
| ReferenceGrant | `gateway.networking.k8s.io/v1` | Standard；Kubernetes 路径仍可探测 `v1beta1` |
| BackendTLSPolicy | `gateway.networking.k8s.io/v1` | 仅支持 v1 |
| TCPRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |
| UDPRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |

“集群 CRD 提供了某个版本”不等于“Edgion 声明支持该版本”。完整支持还需要相应的读取方式、类型转换、语义处理、状态更新和测试。

## Bundle 与 API 版本

仓库当前携带 Gateway API v1.5.0 CRD bundle，而 conformance 测试依赖为 v1.5.1。这两个版本号描述不同的构件，不能合并成一个支持声明。

同一个 GVK，例如 `gateway.networking.k8s.io/v1/HTTPRoute`，在不同 bundle 中可能具有略有差异的 schema。Standard channel 要求兼容演进；Experimental channel 不提供相同保证。

## HTTPRoute v1 内部的功能稳定级别

`v1` 标识约束的是 HTTPRoute API 表示，并不表示其中每个可选功能都已达到稳定状态。在仓库携带的 Gateway API v1.5.0 CRD 中，Experimental channel 会在相同的 `HTTPRoute/v1` GVK 内增加 `spec.useDefaultGateways`、`spec.rules[].retry`、`spec.rules[].sessionPersistence`，以及规则和后端过滤器中的 `externalAuth`。

Edgion 当前已经具备 retry、session persistence 与 ExternalAuth 的模型和运行时支持，但尚未建模 `useDefaultGateways`。这些已实现字段目前也没有全部作为 Gateway API conformance feature 对外声明。因此，资源版本支持、schema 接受、运行时实现和 `GatewayClass.status.supportedFeatures` 是四类独立声明。

这些字段不应被描述成另一个 HTTPRoute alpha API 版本；它们是 v1 表示内部的 Experimental 功能。Extended 也不一定等于 Experimental channel 专属，例如仓库中的 CORS 字段同时存在于两个 channel。

## Kubernetes 模式

Kubernetes API Server 根据集群实际安装的 CRD 执行 schema、默认值、字段裁剪和 CEL 校验。Edgion Controller 随后读取选定 API 版本的对象并执行实现层语义校验。

当前启动检查只确认所需 GVK 是否由集群提供，尚未根据 CRD 的 `gateway.networking.k8s.io/bundle-version` 和 channel 注解建立完整兼容性结论。因此，不应将任意更新后的 Gateway API CRD 视为已经通过 Edgion 验证。

## 非 Kubernetes 模式

文件系统、etcd 和 Admin API 没有 Kubernetes API Server 负责版本转换。未来如果要接受 canonical 版本之外的输入版本，Edgion 必须提供对应 schema、源类型和显式转换；仅仅忽略 `apiVersion` 后反序列化成功，不构成版本支持。

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
