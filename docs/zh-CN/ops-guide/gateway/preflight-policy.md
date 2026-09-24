# Preflight 策略

Edgion 在网关层处理 CORS 预检（`OPTIONS`）请求的方式。

> **🔌 Edgion 扩展**
>
> Preflight 策略通过 `EdgionGatewayConfig` CRD（`spec.preflightPolicy`）配置，不属于标准 Gateway API 规范。

## 概述

浏览器在某些跨域调用之前会发送 CORS 预检 `OPTIONS` 请求。Edgion 可以在边缘处理这些预检，使其无需到达每个后端。默认情况下，若路由配置了 CORS filter，标准预检由该 filter 处理；否则 Edgion 返回 `204 No Content`。CORS filter 也可以按配置将请求转发到上游，详见[与 CORS 的关系](#与-cors-的关系)。

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
| `statusCode` | Integer | `204` | 路由未挂载 CORS 插件时的预检最终响应状态码（200-599） |

### `mode` 取值说明

| 取值 | 含义 |
|------|------|
| `cors-standard` | 严格的 CORS 检测：请求必须是 `OPTIONS` 且同时携带 `Origin` 和 `Access-Control-Request-Method`。符合标准浏览器行为。 |
| `all-options` | 不预先要求 CORS 头，将每个 `OPTIONS` 请求都选入全局预检处理；路由级 CORS filter 仍决定是否接管选中的请求。 |

## 默认行为

Edgion 会自动处理预检；默认 `preflightPolicy` 为 `mode: cors-standard`，`statusCode: 204`。若路由引用了 CORS 插件，预检响应遵循 CORS 规则，包括 `preflightContinue`；否则 Edgion 返回已配置的 `statusCode`。

## 配置场景

### 使用默认配置（无需配置）

大多数部署无需配置 `preflightPolicy`。标准浏览器 CORS 预检会被自动检测并响应。

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
# 无 preflightPolicy 块——使用默认值（mode cors-standard，statusCode 204）
spec: {}
```

### 将所有 OPTIONS 视为预检

当某些客户端发送 `OPTIONS` 请求时不携带标准 CORS 头，并且未配置 CORS filter 的路由需要在本地响应这些请求时，使用 `all-options`。对于配置了 CORS filter 的路由，不带 `Origin` 的 `OPTIONS` 不是 CORS 请求，因此仍会转发到上游。

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

原生 Gateway API `type: CORS` 仅支持配置在 `HTTPRoute.spec.rules[].filters`，
不支持 `backendRefs[].filters`。任何后端上的原生 CORS 都会使所属整条规则无效，
包括零权重后端以及与规则级 CORS 混用的情况。Edgion 通过 `PartiallyInvalid=True`
说明出错的后端索引，其他有效规则继续服务。如果没有规则可以服务，且本路由声明的
规则存在此作用域错误，则改为通过 `Accepted=False`、原因 `UnsupportedValue` 拒绝
Gateway 绑定。对于委派子路由，父路由的委派诊断会说明子规则和后端索引。
预检和实际请求都不再匹配该无效规则；
如果没有其他有效规则匹配，则返回 `404`。请将策略移到规则级，使其应用于该规则的
所有后端。此限制同样适用于委派路由，修改配置后无需重启 Gateway。

原生 CORS 在所有配置后端（包括文件系统输入）均遵循 Gateway API v1.6.2 的准入限制。
Origin 必须使用 `http` 或 `https`，并包含有效主机；可选端口范围为 `1` 至 `65535`。
Origin、方法和允许请求头列表中的通配符不能与其他条目混用；各列表采用集合语义并
遵守上游数量限制。方法、请求头和响应头名称必须使用 Gateway API 接受的值与语法。
`maxAge` 范围为 `1` 至 `2147483647`；省略时默认为 `5`，成功的预检响应会输出
`Access-Control-Max-Age: 5`。非法值会按上文所述使所属规则部分或整体无效。

原生 CORS 设置 `allowCredentials: true` 时，`exposeHeaders` 必须列明响应头名称，
例如 `exposeHeaders: ["X-Request-Id", "X-Total-Count"]`。任一条目等于 `"*"`
（包括与明确名称混用）都会使整条规则无效，并按上述方式报告部分或整条路由拒绝。
`allowCredentials` 为 false 或省略时，`exposeHeaders: ["*"]` 仍输出
`Access-Control-Expose-Headers: *`；允许凭据时，明确名称以逗号分隔输出。
空列表或省略列表不输出该响应头。这是 Edgion 的支持范围限制：Gateway API v1.6.2
禁止凭据请求使用通配符暴露响应头，但没有强制规定拒绝配置；Fetch 在浏览器使用
`credentials: "include"` 时不会展开该通配符。此策略在配置阶段检查，不依据
Cookie 或 Authorization 请求头是否存在作判断。修正列表后，热加载会恢复规则。

预检如何与 CORS 交互取决于路由的 CORS 配置方式：

- **Gateway API 原生 `type: CORS` filter**——预检处理器**不介入**；预检由路由级 filter 链内部处理，`preflightPolicy` 对此路径无效。
- **EdgionPlugins CORS 插件（通过 `ExtensionRef`）**——预检处理器将请求交给正常的路由插件链。对于允许的 Origin，除非设置了 `preflightContinue: true`，标准预检会携带已配置的 CORS 响应头在本地返回；启用该设置后，请求会转发到上游。对于拒绝的 Origin，无论 `preflightContinue` 如何设置，都会在本地返回不带 CORS 响应头的 `204`。不带 `Origin` 的 `OPTIONS` 不是 CORS 请求，即使 `mode: all-options` 将其选入全局处理，CORS 插件也不会接管，而是将其转发到上游。插件 entry-level conditions 仍然生效；条件不匹配时会跳过 CORS 插件，并将请求继续转发到上游。
- **没有路由级 CORS filter**——`preflightPolicy` 选中的请求会使用 `statusCode` 在本地响应，不携带 CORS 响应头。

完整行为如下：

| 请求 | 路由 CORS filter | 结果 |
|------|------------------|------|
| 标准预检（`OPTIONS` + `Origin` + `Access-Control-Request-Method`） | 无 | 本地返回 `statusCode`，不携带 CORS 响应头 |
| 标准预检，Origin 被允许 | Gateway API 原生 CORS | 本地返回 `204`，携带 CORS 响应头 |
| 标准预检，Origin 被拒绝 | Gateway API 原生 CORS | 本地返回 `204`，不携带 CORS 响应头 |
| 标准预检，Origin 被允许 | EdgionPlugins CORS，`preflightContinue: false` | 本地返回 `204`，携带 CORS 响应头 |
| 标准预检，Origin 被允许 | EdgionPlugins CORS，`preflightContinue: true` | 转发到上游，并在上游响应中添加 CORS 响应头 |
| 标准预检，Origin 被拒绝 | EdgionPlugins CORS，任意 `preflightContinue` 值 | 本地返回 `204`，不携带 CORS 响应头 |
| 标准预检 | entry-level conditions 不匹配的 EdgionPlugins CORS | 转发到上游，不添加 CORS 响应头 |
| 不带 `Origin` 的 `OPTIONS`，`mode: all-options` | 无 | 本地返回 `statusCode`，不携带 CORS 响应头 |
| 不带 `Origin` 的 `OPTIONS`，`mode: all-options` | 原生或 EdgionPlugins CORS | 作为非 CORS 请求转发到上游 |
| 带 `Origin` 但不带 `Access-Control-Request-Method` 的 `OPTIONS`，`mode: all-options` | 无 | 本地返回 `statusCode`，不携带 CORS 响应头 |
| 带 `Origin` 但不带 `Access-Control-Request-Method` 的 `OPTIONS`，`mode: all-options` | 原生或 EdgionPlugins CORS | 作为实际 CORS 请求转发；若 Origin 被允许，则在上游响应中添加 CORS 响应头 |

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
