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
    - enable: true
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
| `debugHeader` | Boolean | ❌ | `false` | 是否在响应中添加调试 header |

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

### 场景 2：调试模式

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-IP
      debugHeader: true
      onMissing: fallback
```

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
- `debugHeader: true` 时，响应中会包含 `X-Direct-Endpoint` header 指示实际路由的 endpoint
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

**原因**：`onInvalid` 设置为 `Reject` 且提供的值格式无效。

**解决方案**：确保提供的是合法的 IP 地址格式。

---

## 相关文档

- [Dynamic Upstream](./dynamic-upstream.md)
- [负载均衡算法](../../lb-algorithms.md)
- [后端配置](../../backends/README.md)
