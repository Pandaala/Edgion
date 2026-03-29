# Preflight 请求处理策略

> **🔌 Edgion 扩展**
> 
> Preflight 策略通过 `EdgionGatewayConfig` CRD 配置，是 Edgion 的扩展功能。

## 什么是 Preflight？

Preflight（预检请求）是浏览器在发送跨域请求前自动发起的 OPTIONS 请求，用于检查服务器是否允许实际请求。

**示例**：
- 浏览器要发送 `POST` 请求到 `https://api.example.com`
- 浏览器先自动发送 `OPTIONS` 请求检查权限
- 服务器返回允许的方法和头部信息
- 浏览器再发送实际的 `POST` 请求

## 默认行为

Edgion Gateway 会**自动处理所有 preflight 请求**，无需额外配置。

- 如果路由配置了 CORS 插件，使用 CORS 配置响应
- 如果没有 CORS 插件，返回 `204 No Content`

## 自定义配置（可选）

如果需要自定义 preflight 处理行为，可在 `EdgionGatewayConfig` 中配置：

```yaml
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: edgion-gateway-config
spec:
  preflightPolicy:
    mode: cors-standard        # 或 all-options
    statusCode: 204            # 默认状态码
```

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `mode` | String | `cors-standard` | Preflight 检测模式，见下文 |
| `statusCode` | Integer | `204` | 未配置 CORS 时的默认响应状态码 |

### mode 选项

#### `cors-standard`（推荐）

符合 CORS 标准的严格检测：

- 请求方法必须是 `OPTIONS`
- 必须包含 `Origin` 头
- 必须包含 `Access-Control-Request-Method` 头

**适用场景**：标准的浏览器跨域请求

#### `all-options`

将所有 `OPTIONS` 请求都视为 preflight：

- 只要请求方法是 `OPTIONS` 就处理
- 不检查其他头部

**适用场景**：
- 某些非标准客户端
- 需要统一处理所有 OPTIONS 请求

## 使用示例

### 场景 1：使用默认配置（无需配置）

大多数情况下，默认配置即可满足需求，无需任何配置。

### 场景 2：自定义 preflight 检测模式

```yaml
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: edgion-gateway-config
spec:
  preflightPolicy:
    mode: all-options
```

### 场景 3：自定义默认状态码

```yaml
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: edgion-gateway-config
spec:
  preflightPolicy:
    statusCode: 200
```

## 与 CORS 的关系

Preflight 策略的行为取决于路由使用的 CORS 配置方式：

### Gateway API 原生 CORS filter（`type: CORS`）

当路由配置了原生 `type: CORS` filter 时，**preflight handler 不介入**，preflight 请求直接由路由级插件 `GapiCorsFilter` 处理。`preflightPolicy` 配置对此路径无效。

### EdgionPlugins CORS 插件

当路由通过 `ExtensionRef` 引用 EdgionPlugins 中的 CORS 插件时，preflight handler **协同工作**：

1. **Preflight Handler 先执行**：在所有插件之前拦截 preflight 请求
2. **自动查找 CORS 配置**：从路由的 EdgionPlugins 中查找 CORS 插件配置
3. **使用 CORS 响应**：如果找到 CORS 配置，按 CORS 规则响应
4. **默认响应**：如果没有 CORS 配置，返回配置的默认状态码

**示例流程**：

```
浏览器 OPTIONS 请求
    ↓
路由配置了原生 type: CORS filter？
    ├─ 是 → 跳过 Preflight Handler，由 GapiCorsFilter 处理
    └─ 否 → Preflight Handler 拦截
              ↓
          路由有 EdgionPlugins CORS 配置？
              ├─ 是 → 使用 CORS 配置响应
              └─ 否 → 返回 204 No Content
```

## 常见问题

### Q: 需要配置 preflight 吗？

**A:** 大多数情况下不需要。默认配置已经能正确处理标准的 CORS preflight 请求。

### Q: 什么时候需要使用 `all-options` 模式？

**A:** 当你遇到以下情况：
- 某些客户端发送的 OPTIONS 请求不包含标准的 CORS 头部
- 需要统一处理所有 OPTIONS 请求，不管是否是 CORS preflight

### Q: Preflight 响应会经过其他插件吗？

**A:** 取决于 CORS 配置方式：
- **EdgionPlugins CORS**：不会。Preflight 在所有插件之前由 preflight handler 拦截并响应。
- **原生 `type: CORS` filter**：Preflight 在路由级插件链中由 GapiCorsFilter 处理，HTTPRoute `filters` 数组中排在 CORS 之前的 filter 会先执行。建议将 CORS filter 放在 filters 列表首位。

### Q: 如何查看 preflight 请求的处理日志？

**A:** Preflight 请求会记录在 `edgion_access.log` 中，可以通过 HTTP 方法 `OPTIONS` 进行过滤：

```bash
grep "OPTIONS" logs/edgion_access.log
```

## 相关文档

- [CORS 插件配置](../../user-guide/http-route/filters/edgion-plugins/cors.md)
- [资源架构总览（开发指南）](../../dev-guide/resource-architecture-overview.md)
