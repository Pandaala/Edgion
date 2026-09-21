# GatewayClass

绑定 Edgion 控制器，并可选择指定调优参数集。

## 概述

`GatewayClass` 是集群作用域的资源，用于告知 Kubernetes 哪个控制器管理一类 Gateway。要在 Edgion 上运行 Gateway，需要创建一个 `controllerName` 与 Edgion 控制器匹配的 GatewayClass，然后在 Gateway 上设置 `gatewayClassName` 引用它。GatewayClass 还可以通过 `parametersRef` 指向 `EdgionGatewayConfig` 来调优数据面行为。

Edgion 采用 1 控制器 : 1 GatewayClass 模型：单个 Edgion 实例管理一个 GatewayClass。`parametersRef` 是可选的——未设置时 Gateway 使用内置默认值；设置时必须引用 `edgion.io/EdgionGatewayConfig`。

## 配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
```

## 字段参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `spec.controllerName` | String | 是 | 必须匹配 Controller 配置的 `controller_name`，默认值为 `edgion.io/gateway-controller`。 |
| `spec.parametersRef` | Object | 否 | 指向调优网关行为的 `EdgionGatewayConfig` 的引用。设置后 `group` 必须为 `edgion.io`，`kind` 必须为 `EdgionGatewayConfig`，否则 GatewayClass 将被拒绝（`Accepted: False`）。 |
| `spec.parametersRef.group` | String | 否 | 参数资源的 API group：`edgion.io`。 |
| `spec.parametersRef.kind` | String | 否 | `EdgionGatewayConfig`。 |
| `spec.parametersRef.name` | String | 否 | `EdgionGatewayConfig` 资源的名称。 |
| `spec.description` | String | 否 | 人类可读的描述。 |

`EdgionGatewayConfig` 是集群作用域的，因此 `parametersRef` 不包含 `namespace` 字段。

## controllerName

Edgion 控制器名称的默认值为：

```
edgion.io/gateway-controller
```

Kubernetes 模式可以通过 Controller 配置中的 `controller_name` 修改此名称；GatewayClass 必须匹配实际配置值。

Kubernetes 所有权同时检查 `controller_name` 和配置的 `gateway_class`。Controller 只有在已发布匹配的 GatewayClass 实例后才处理 Gateway，原始对象与已发布对象必须具有相同且非空的 Kubernetes UID。缺失、属于其他控制器或未监听的 class 不能授权处理 Gateway；因此 `no_watch_kinds: [GatewayClass]` 会停止处理 Gateway。class 删除或替换时，相关 Gateway 缓存会撤销，待所有权重新成立后恢复。class 的 `Accepted=False` 本身不转移所有权，普通 generation 更新也不会撤销所有权。

独立部署应使用不同的 GatewayClass。共享同一 class 和 controller name 的 HA 副本必须共享选举域及配置、资源视图。下述版本校验不能阻止两套独立选主且管理范围重叠的部署交替写入不同的期望状态。

已知限制：不同 class 尚不能隔离 EdgionAcme 的状态写入者。如果两套独立选主的部署都监听同一个 EdgionAcme，但只有一套发布了它引用的 Gateway，两者可能交替写入相反的 `ResolvedRefs`。在补齐 ACME 所有权准入前，应避免独立部署的 EdgionAcme 监听范围重叠；仅使用不同 controller name 也不能阻止此冲突。

排查 Route 挂载时，同时检查 `status.parents[].parentRef` 和 `controllerName`。Gateway 只使用当前 Edgion 控制器对应条目的 `Accepted` 结果；其他控制器的 `Accepted=True` 不会覆盖 Edgion 的 `Accepted=False`。不同资源类型、Listener section 或端口的父引用分别判断。Controller 会保留读取到的其他控制器状态，并清理自己的过期或重复条目。

Kubernetes 模式下，Controller 的通用状态更新会检查计算所依据的资源版本。如果其他写者先更新了资源，陈旧写入会以 `409 Conflict` 失败，保留已成功写入的新状态。Controller 会记录失败，但不立即重试；后续资源事件、相关依赖变化或成功重新 LIST 可以触发再次协调。状态恢复没有固定时限；如果状态持续不更新，请检查 Controller 的状态写入告警及 API 权限。

## 配置场景

### 无参数的基础 class

最简单的配置。引用此 class 的 Gateway 将使用 Edgion 默认值。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  description: "Edgion Gateway powered by Pingora"
```

### 带调优参数的 class

> **🔌 Edgion 扩展**
>
> 通过 `parametersRef` 引用 `EdgionGatewayConfig` CRD 是 Edgion 扩展功能，不属于标准 Gateway API。

挂载 `EdgionGatewayConfig` 以调优该 class 中所有 Gateway 的数据面行为。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: prod-gateway-config
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: prod-gateway-config
spec:
  server:
    enableCompression: true
    downstreamKeepaliveRequestLimit: 1000
  httpTimeout:
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
```

完整可调字段详见 [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)。

### 无需重建 Gateway 即可切换参数

由于 Gateway 通过名称引用 class，可以将 `parametersRef` 指向不同的 `EdgionGatewayConfig`。动态解析的策略会应用于后续请求或连接；`spec.server` 下用于构造 listener 的字段需要重启 Gateway 进程。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: tuned-gateway-config
```

## 边界情况与注意事项

### 共享资源的部署状态

Kubernetes 中，`EdgionPlugins`、`EdgionStreamPlugins`、`EdgionBackend` 和
`LinkSys` 将每个部署的完整观察结果分别放在 `status.controllers[]` 中。
请按 `controllerName` 和 `gatewayClassName` 选择条目，再读取其 `status.conditions`；
`gatewayClassUid` 标识当前 GatewayClass 实例。LinkSys 的凭据生命周期字段也在该条目的
`status` 内，不再读取顶层 `status.conditions` 或顶层生命周期字段。

每个资源最多保存 32 个部署条目。达到上限时，已有条目仍可更新，新部署不会挤掉其他条目；
Controller 会报告无法写入状态，本地配置处理不受影响。停用部署的条目不会按时间自动清除，
只有观察到 GatewayClass 删除或实例替换等明确证据后才清理。FileSystem 和 etcd
仍按原生状态格式持久化。
Gateway 收到的配置会保留完整 `status`，并包含由 Controller 生成的
`spec.currentStatus`。排查当前部署的路由接纳、Listener 状态或 ACME challenge 时，
应查看这份当前状态；其他部署的成功状态不会授权当前部署提供服务。
该字段是内部诊断和传输内容，不要写入资源 YAML。Route 缺少当前状态或对应的
`Accepted=True` 条目时，不会被数据面接纳。不同独立部署应使用不同的 GatewayClass；
共用同一个 class 的副本必须共享选主域和配置视图。

### GatewayClass 注意事项

- `controllerName` 在 Gateway API 中是不可变的；如需更换控制器，必须删除并重新创建 GatewayClass。
- 如果 `controllerName` 与 Controller 配置的名称不完全匹配，Edgion 永远不会设置 `Accepted` 状态，引用该 class 的 Gateway 将保持未编程状态。
- `parametersRef` 的 `group` 或 `kind` 错误时会以 `Accepted: False` 拒绝；未设置 `parametersRef` 是有效的，将回退到默认值。
- `EdgionGatewayConfig` 是集群作用域的，因此 `parametersRef` 不包含 `namespace`。
- 共享的 `EdgionGatewayConfig.status` 只报告配置自身的合法性。可见的 `globalPluginsRef` 若包含不允许在早期阶段使用的 RequestId 来源或条件，错误记录在消费该配置的 GatewayClass 上：`Accepted=False/InvalidParameters`；`ResolvedRefs=True` 仍表示 EGC 存在。请在 GatewayClass 状态中查看此类部署局部错误；该检查不会新增对不可见或未观察插件的拒绝。
- `EdgionGatewayConfig` 的更改动态生效（无需重启 Gateway）；详见其页面中关于热重载的字段说明。

## 相关文档

- [Gateway 总览](overview.md)
- [监听器](listeners/README.md)
- [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)
- [Preflight 策略](preflight-policy.md)
