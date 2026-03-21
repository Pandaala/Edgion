---
name: gateway-class-resource
description: GatewayClass 资源 Schema：控制器绑定和运行时配置引用。
---

# GatewayClass 资源

> API: `gateway.networking.k8s.io/v1` | Scope: Cluster
> Gateway API v1.4.0 Core 资源

GatewayClass 定义了 Gateway 的"类型"，关联到特定的控制器实现，并可引用运行时配置。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller   # 必填：控制器标识
  parametersRef:                                   # 可选：运行时配置引用
    group: edgion.io
    kind: EdgionGatewayConfig
    name: default-config
  description: "Edgion Gateway powered by Pingora" # 可选：描述
```

## spec 字段

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `controllerName` | `String` | 是 | 控制器标识（Edgion 固定为 `edgion.io/gateway-controller`） |
| `parametersRef` | `ParametersRef?` | 否 | 引用 EdgionGatewayConfig CRD |
| `description` | `String?` | 否 | 人类可读描述 |

### parametersRef Schema

```yaml
parametersRef:
  group: String      # "edgion.io"
  kind: String       # "EdgionGatewayConfig"
  name: String       # EdgionGatewayConfig 资源名称
```

## 与 Gateway 的关系

```
GatewayClass (cluster-scoped)
  ├── controllerName: 标识由哪个控制器处理
  ├── parametersRef → EdgionGatewayConfig (运行时配置)
  │
  └── Gateway.spec.gatewayClassName: "edgion"
       ├── Listener 1 (port, protocol, tls)
       ├── Listener 2
       └── ...
```

- 一个 GatewayClass 可对应多个 Gateway
- Controller 在 K8s 模式下通过 `conf_center.gateway_class` 配置匹配的 GatewayClass 名称
- FileSystem 模式下忽略 GatewayClass 过滤

## Status Schema

```yaml
status:
  conditions:
    - type: Accepted                    # 控制器已接受此 GatewayClass
      status: "True"
      reason: Accepted
    - type: SupportedVersion            # API 版本支持
      status: "True"
```
