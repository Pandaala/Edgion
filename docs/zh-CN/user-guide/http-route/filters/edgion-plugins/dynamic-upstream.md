# Dynamic Upstream 插件

> **🔌 Edgion 扩展**
> 
> DynamicInternalUpstream 和 DynamicExternalUpstream 是 `EdgionPlugins` CRD 提供的动态上游路由插件，不属于标准 Gateway API。

## 概述

Dynamic Upstream 允许根据请求中的元数据（Header / Query / Cookie 等）动态选择上游目标。包含两个子类型：

- **DynamicInternalUpstream**：在当前路由已有的 `backendRefs` 中动态选择特定 Service，绕过加权选择
- **DynamicExternalUpstream**：将流量路由到外部域名，通过域名映射白名单控制

## DynamicInternalUpstream

### 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: diu-plugin
spec:
  requestPlugins:
    - enable: true
      type: DynamicInternalUpstream
      config:
        from:
          type: header
          name: X-Backend-Target
        onMissing: fallback
```

### 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `from` | Object | ✅ | `{type:"header",name:"X-Backend-Target"}` | 值来源 |
| `from.type` | String | ✅ | `header` | 来源类型：`header` / `query` / `cookie` / `ctx` |
| `from.name` | String | ✅ | 无 | 来源名称 |
| `extract` | Object | ❌ | 无 | 提取规则，包含 `regex` 和 `group` |
| `rules` | Array | ❌ | 无 | 匹配规则列表，不设置则使用 direct 模式 |
| `onMissing` | String | ❌ | `Fallback` | 值缺失时行为：`Fallback` / `Reject` |
| `onNoMatch` | String | ❌ | `Fallback` | 无匹配规则时行为：`Fallback` / `Reject` |
| `onInvalid` | String | ❌ | `Reject` | 值无效时行为：`Reject` / `Fallback` |
| `debugHeader` | Boolean | ❌ | `false` | 是否添加调试 header |

### 场景示例

#### Direct 模式

Header 值直接作为目标 Service 名称：

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: header
        name: X-Backend-Target
      onMissing: fallback
      debugHeader: true
```

```bash
curl -H "X-Backend-Target: service-v2" https://api.example.com/api
```

#### Rules 模式

通过规则映射 Header 值到目标 Service：

```yaml
requestPlugins:
  - type: DynamicInternalUpstream
    config:
      from:
        type: header
        name: X-Version
      rules:
        - match: "v1"
          target: "api-v1"
        - match: "v2"
          target: "api-v2"
        - match: "canary"
          target: "api-canary"
      onNoMatch: fallback
```

---

## DynamicExternalUpstream

### 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: deu-plugin
spec:
  requestPlugins:
    - enable: true
      type: DynamicExternalUpstream
      config:
        from:
          type: header
          name: X-Target-Region
        domainMap:
          "us-west":
            domain: us-west.api.internal
            port: 443
            tls: true
          "eu-central":
            domain: eu-central.api.internal
            port: 443
            tls: true
```

### 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `from` | Object | ✅ | `{type:"header",name:"X-Target-Region"}` | 值来源 |
| `extract` | Object | ❌ | 无 | 提取规则 |
| `domainMap` | Object | ✅ | 无 | 域名映射白名单 |
| `onMissing` | String | ❌ | `Skip` | 值缺失时行为：`Skip` / `Reject` |
| `onNoMatch` | String | ❌ | `Skip` | 无匹配时行为：`Skip` / `Reject` |
| `debugHeader` | Boolean | ❌ | `false` | 是否添加调试 header |

### DomainTarget 子字段

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `domain` | String | ✅ | 无 | 目标域名 |
| `port` | Integer | ❌ | 无 | 目标端口 |
| `tls` | Boolean | ❌ | `true` | 是否使用 TLS |
| `overrideHost` | String | ❌ | 无 | 覆盖 Host header |
| `sni` | String | ❌ | 无 | TLS SNI 名称 |

### 场景示例

#### 多区域路由

```yaml
requestPlugins:
  - type: DynamicExternalUpstream
    config:
      from:
        type: header
        name: X-Cluster-Target
      extract:
        regex: 'cluster=([\w-]+)'
        group: 1
      domainMap:
        "us-west":
          domain: us-west.api.internal
          port: 443
          tls: true
          overrideHost: api.example.com
        "eu-central":
          domain: eu-central.api.internal
          port: 443
          tls: true
          overrideHost: api.example.com
        "ap-east":
          domain: ap-east.api.internal
          port: 443
          tls: true
      onMissing: skip
      debugHeader: true
```

---

## 行为细节

- **DynamicInternalUpstream**：选择的目标必须存在于当前路由的 `backendRefs` 中，否则视为无效
- **DynamicExternalUpstream**：只能路由到 `domainMap` 白名单中的域名，防止 SSRF
- 两个插件的 `onMissing: Skip` / `Fallback` 模式下，未匹配时使用正常的路由逻辑
- `debugHeader: true` 时会在响应中添加 header 指示实际路由的上游

---

## 相关文档

- [Direct Endpoint](./direct-endpoint.md)
- [负载均衡算法](../../lb-algorithms.md)
- [ProxyRewrite](./proxy-rewrite.md)
