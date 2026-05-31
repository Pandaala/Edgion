# Preflight 策略

Edgion 在网关层处理 CORS 预检（`OPTIONS`）请求的方式。

> **🔌 Edgion 扩展**
>
> Preflight 策略通过 `EdgionGatewayConfig` CRD（`spec.preflightPolicy`）配置，不属于标准 Gateway API 规范。

## 概述

浏览器在某些跨域调用之前会发送 CORS 预检 `OPTIONS` 请求。Edgion 在边缘处理这些预检，使其无需到达每个后端。默认情况下无需任何配置即可工作：如果路由配置了 CORS 插件，Edgion 会使用该 CORS 配置来响应；否则返回 `204 No Content`。

只有当您想改变*哪些* `OPTIONS` 请求被视为预检，或修改默认状态码时，才需要配置 `preflightPolicy`。它在 GatewayClass 引用的 `EdgionGatewayConfig` 上设置，而非通过注解。

## 配置

`preflightPolicy` 是 `EdgionGatewayConfig.spec` 的字段。通过 GatewayClass 的 `parametersRef` 引用该配置：

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
    name: default-config
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  preflightPolicy:
    # 哪些 OPTIONS 请求被视为预检
    mode: cors-standard
    # 路由未挂载 CORS 插件时使用的默认状态码
    statusCode: 204
```

## 字段参考

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `mode` | Enum | `cors-standard` | 哪些 `OPTIONS` 请求被视为预检。可选值：`cors-standard` \| `all-options` |
| `statusCode` | Integer | `204` | 路由未挂载 CORS 插件时的预检响应状态码 |

### `mode` 取值说明

| 取值 | 含义 |
|------|------|
| `cors-standard` | 严格的 CORS 检测：请求必须是 `OPTIONS` 且同时携带 `Origin` 和 `Access-Control-Request-Method`。符合标准浏览器行为。 |
| `all-options` | 将所有 `OPTIONS` 请求都视为预检，不检查 CORS 头。适用于省略 CORS 头的非标准客户端。 |

## 默认行为

Edgion 会自动处理预检；默认 `preflightPolicy` 为 `mode: cors-standard`，`statusCode: 204`。若路由引用了 CORS 插件，预检响应遵循 CORS 规则；否则 Edgion 返回已配置的 `statusCode`。

## 配置场景

### 使用默认配置（无需配置）

大多数部署无需配置 `preflightPolicy`。标准浏览器 CORS 预检会被自动检测并响应。

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  # 无 preflightPolicy 块——使用默认值（mode cors-standard，statusCode 204）
  server:
    threads: 0
```

### 将所有 OPTIONS 视为预检

当某些客户端发送 `OPTIONS` 请求时不携带标准 CORS 头，使用 `all-options`。

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  preflightPolicy:
    mode: all-options
```

### 自定义默认状态码

当客户端对空的 `204` 响应处理有误时，改用 `200` 响应。

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  preflightPolicy:
    mode: cors-standard
    statusCode: 200
```

## 与 CORS 的关系

预检如何与 CORS 交互取决于路由的 CORS 配置方式：

- **Gateway API 原生 `type: CORS` filter**——预检处理器**不介入**；预检由路由级 filter 链内部处理，`preflightPolicy` 对此路径无效。
- **EdgionPlugins CORS 插件（通过 `ExtensionRef`）**——预检处理器先运行，找到路由的 CORS 配置后使用该配置响应。若未找到 CORS 配置，则返回已配置的 `statusCode`。

## 边界情况与注意事项

- `preflightPolicy` 在 `EdgionGatewayConfig` 中，而非 Gateway 注解。在 Gateway 上设置无效。
- 策略对引用该 `EdgionGatewayConfig` 的整个 GatewayClass 生效，而非单个 Gateway 或 Listener。
- 此策略仅控制 `OPTIONS` 的短路处理，不会在实际跨域响应中添加 `Access-Control-Allow-*` 头；这部分仍由 CORS 插件或后端负责。
- 更新 `EdgionGatewayConfig` 后变更动态生效，无需重启 Gateway。

## 相关文档

- [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)
- [GatewayClass](gateway-class.md)
- [CORS 插件](../../user-guide/http-route/filters/edgion-plugins/cors.md)
- [Gateway 总览](overview.md)
