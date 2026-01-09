# GatewayClass 配置

GatewayClass 定义了 Gateway 的实现类型，类似于 IngressClass。

> **🔌 Edgion 扩展**
> 
> `parametersRef` 可引用 `EdgionGatewayConfig` CRD 进行高级配置，这是 Edgion 扩展功能。

## 资源结构

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:              # 可选：引用配置参数
    group: edgion.io
    kind: EdgionGatewayConfig
    name: default-config
```

## 配置参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| controllerName | string | ✓ | 控制器标识 |
| parametersRef | object | | 配置参数引用 |
| description | string | | 描述信息 |

## Edgion 控制器名称

Edgion 使用的控制器名称：

```yaml
controllerName: edgion.io/gateway-controller
```

## 示例

### 示例 1: 基本配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
```

### 示例 2: 带参数配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion-custom
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: custom-config
---
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: custom-config
spec:
  server:
    threads: 4
    gracePeriodSeconds: 30
```

## 相关文档

- [Gateway 总览](./overview.md)
