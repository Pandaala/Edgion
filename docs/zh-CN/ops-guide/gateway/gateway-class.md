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

排查 Route 挂载时，同时检查 `status.parents[].parentRef` 和 `controllerName`。Gateway 只使用当前 Edgion 控制器对应条目的 `Accepted` 结果；其他控制器的 `Accepted=True` 不会覆盖 Edgion 的 `Accepted=False`。不同资源类型、Listener section 或端口的父引用分别判断。Controller 会保留读取到的其他控制器状态，并清理自己的过期或重复条目。

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

- `controllerName` 在 Gateway API 中是不可变的；如需更换控制器，必须删除并重新创建 GatewayClass。
- 如果 `controllerName` 与 Controller 配置的名称不完全匹配，Edgion 永远不会设置 `Accepted` 状态，引用该 class 的 Gateway 将保持未编程状态。
- `parametersRef` 的 `group` 或 `kind` 错误时会以 `Accepted: False` 拒绝；未设置 `parametersRef` 是有效的，将回退到默认值。
- `EdgionGatewayConfig` 是集群作用域的，因此 `parametersRef` 不包含 `namespace`。
- `EdgionGatewayConfig` 的更改动态生效（无需重启 Gateway）；详见其页面中关于热重载的字段说明。

## 相关文档

- [Gateway 总览](overview.md)
- [监听器](listeners/README.md)
- [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)
- [Preflight 策略](preflight-policy.md)
