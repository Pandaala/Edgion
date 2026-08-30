# Direct Endpoint 插件

> **🔌 Edgion 扩展**
> 
> DirectEndpoint 是 `EdgionPlugins` CRD 提供的直接端点路由插件，不属于标准 Gateway API。

## 概述

Direct Endpoint 允许通过请求中的元数据（Header / Query / Cookie 等）指定目标 endpoint IP，绕过负载均衡算法直接路由到特定后端实例。指定的 endpoint 必须属于当前路由的 `backendRefs` 中的某个 Service。

适用于调试、定向测试、会话亲和等场景。

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: direct-endpoint-plugin
spec:
  requestPlugins:
      type: DirectEndpoint
      config:
        from:
          type: header
          name: X-Target-Endpoint
```

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `from` | Object | ✅ | `{type:"header",name:"X-Target-Endpoint"}` | 目标 endpoint 值的来源 |
| `from.type` | String | ✅ | `header` | 来源类型：`header` / `query` / `cookie` / `ctx` |
| `from.name` | String | ✅ | `X-Target-Endpoint` | 来源名称 |
| `extract` | Object | ❌ | 无 | 提取规则 |
| `extract.regex` | String | ✅ | 无 | 正则表达式 |
| `extract.group` | Integer | ✅ | 无 | 捕获组序号 |
| `port` | Integer | ❌ | 无 | 覆盖端口号 |
| `onMissing` | String | ❌ | `Fallback` | 值缺失时行为：`Fallback`（回退正常LB）/ `Reject`（拒绝请求）|
| `onInvalid` | String | ❌ | `Reject` | 值无效时行为：`Reject` / `Fallback` |
| `inheritTls` | Boolean | ❌ | `true` | 是否继承路由的 TLS 配置 |

`dye` 是**条目级（entry-level）**字段——与 `type`/`config`/`enable` 同级，不属于 `config` 的成员：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `dye` | Object | 无 | 按结果分类的流量染色请求头/响应头（`on: [success, fallback, reject]`）。`request`/`response` 为 `{name, on, value, conditions}` 条目列表；不设置 `value` 时，`success` 上携带已解析的 endpoint `ip:port`。完整参考见 [染色（Dye）](./dye.md)，插件特有内容见下方[场景 2：染色请求头](#场景-2染色请求头) |

---

## 常见配置场景

### 场景 1：通过 Header 指定目标实例

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-IP
      onMissing: fallback
      onInvalid: reject
```

**测试**：
```bash
curl -H "X-Target-IP: 10.0.1.5" https://api.example.com/debug
```

### 场景 2：染色请求头

完整参考见 [染色（Dye）](./dye.md)：`on` 结果词表、两种 `value` 来源、模板引擎、防伪造剥离（即使插件被 `enable: false` 关闭或被 conditions 跳过也会执行）、请求头名校验，以及安全指引。本节只覆盖 DirectEndpoint 特有的部分。

```yaml
requestPlugins:
  - type: DirectEndpoint
    dye:
      response:
        - name: X-Direct-Endpoint
        - name: X-Edgion-Outcome
          on: [fallback, reject]     # 不设置 value => direct_endpoint:fallback:on-missing 等
    config:
      from:
        type: header
        name: X-Target-IP
      onMissing: fallback
```

`dye` 默认关闭：不配置时插件不设置任何请求头，也不做任何剥离。配置后，会附加运维自定义的 HTTP 头——`request` 条目设置在转发给上游的请求上，`response` 条目设置在返回给客户端的**响应**上（与上面 `X-Direct-Endpoint` 这个惯用头的语义一致，它历来就是下游响应头，而不是请求头）。`success` 上不设置 `value` 时携带已解析的 endpoint `ip:port`：

```bash
curl -H "X-Target-IP: 10.0.1.5" https://api.example.com/debug  # 客户端在响应上看到 X-Direct-Endpoint: 10.0.1.5:8080
```

DirectEndpoint 支持 `success`、`fallback`、`reject` 三种染色能力——默认 `on`（即 `success`）只在 endpoint 成功匹配到路由后端之后才会染色；`fallback`/`reject` 规则则会分别给 `onMissing`（值缺失）和 `onInvalid`（endpoint 不属于任何 backendRef）分支染色。

### 场景 3：正则提取 endpoint

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-Info
      extract:
        regex: 'ip=([0-9.]+)'
        group: 1
      port: 8080
```

---

## 行为细节

- 指定的 endpoint IP 必须属于当前路由 `backendRefs` 中某个 Service 的 Endpoints 列表
- 如果 endpoint 不属于任何 backend，视为 `onInvalid` 处理
- `dye` 默认关闭；默认 `on`（即 `success`）规则只在成功解析出 endpoint 后才会应用，`fallback`/`reject` 规则则应用于对应的 `onMissing`/`onInvalid` 分支——见上方[场景 2：染色请求头](#场景-2染色请求头)
- `Fallback` 模式下回退到正常的负载均衡选择

---

## 故障排除

### 问题 1：指定 IP 但路由到其他实例

**原因**：指定的 IP 不属于当前路由的 backend Service。

**解决方案**：
```bash
# 检查 Service 的 Endpoints
kubectl get endpoints <service-name> -o yaml
```

### 问题 2：返回 400 错误

**原因**：请求中缺少来源值（endpoint 值），且 `onMissing` 设置为 `Reject`。

**解决方案**：提供配置的来源（Header/Query/Cookie/Ctx），或将 `onMissing` 设为 `fallback`，以便在值缺失时回退到正常负载均衡。

### 问题 3：返回 403 错误

**原因**：解析出的端点不属于路由任何 `backendRefs` 的后端，且 `onInvalid` 为 `Reject`（默认值）。

**解决方案**：确认该端点 IP 属于某个后端 Service 的 Endpoint，或将 `onInvalid` 设为 `fallback`。

---

## 相关文档

- [Dynamic Upstream](./dynamic-upstream.md)
- [后端配置](../../backends/README.md)
- [过滤器总览](../overview.md)
