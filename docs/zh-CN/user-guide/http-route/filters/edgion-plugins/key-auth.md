# Key Auth 插件

> **🔌 Edgion 扩展**
> 
> KeyAuth 是 `EdgionPlugins` CRD 提供的 API Key 认证插件，不属于标准 Gateway API。

## 概述

Key Auth（API Key 认证）通过验证请求中携带的 API Key 来控制访问权限。支持从 Header、Query 参数、Cookie 等多种来源获取 Key。

**工作原理**：
1. 从配置的来源（Header / Query / Cookie）中提取 API Key
2. 与 Kubernetes Secret 中存储的有效 Key 进行比对
3. 验证成功：允许访问，可选地将 Key 元数据传递给上游
4. 验证失败：返回 401 状态码

## 快速开始

### 创建 API Key Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: api-keys
  namespace: default
type: Opaque
stringData:
  keys.yaml: |
    - key: "my-secret-api-key-1"
      username: "user1"
    - key: "my-secret-api-key-2"
      username: "user2"
```

### 配置插件

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: key-auth-plugin
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: KeyAuth
      config:
        keySources:
          - type: header
            name: "X-API-Key"
        secretRefs:
          - name: api-keys
```

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `keySources` | Array | ❌ | `[{type:"header",name:"apikey"}, {type:"query",name:"apikey"}]` | Key 来源列表 |
| `hideCredentials` | Boolean | ❌ | `false` | 验证后是否从请求中移除 API Key |
| `authFailureDelayMs` | Integer | ❌ | `0` | 认证失败后延迟响应毫秒数，防暴力破解 |
| `anonymous` | String | ❌ | 无 | 匿名用户名，设置后未认证请求也允许通过 |
| `realm` | String | ❌ | `"API Gateway"` | 认证域名称 |
| `keyField` | String | ❌ | `"key"` | Secret 中存储 key 值的字段名 |
| `secretRefs` | Array | ✅ | 无 | Kubernetes Secret 引用列表 |
| `upstreamHeaderFields` | Array | ❌ | `[]` | 认证成功后传递给上游的额外 header（从 key metadata 提取） |

### KeySource 子字段

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `type` | String | ✅ | 来源类型：`header` / `query` / `cookie` / `ctx` |
| `name` | String | ✅ | 来源名称（如 header 名、query 参数名） |

---

## 常见配置场景

### 场景 1：从 Header 获取 API Key

```yaml
requestPlugins:
  - enable: true
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      secretRefs:
        - name: api-keys
      hideCredentials: true
```

**测试**：
```bash
curl -H "X-API-Key: my-secret-api-key-1" https://api.example.com/resource
```

### 场景 2：支持多种来源

同时支持从 Header、Query 参数和 Cookie 获取 Key：

```yaml
requestPlugins:
  - enable: true
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
        - type: query
          name: api_key
        - type: cookie
          name: api_key
      secretRefs:
        - name: api-keys
```

**测试**：
```bash
# 通过 Header
curl -H "X-API-Key: my-key" https://api.example.com/resource

# 通过 Query 参数
curl "https://api.example.com/resource?api_key=my-key"
```

### 场景 3：匿名访问模式

```yaml
requestPlugins:
  - enable: true
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      secretRefs:
        - name: api-keys
      anonymous: "guest"
```

### 场景 4：传递用户元数据到上游

```yaml
requestPlugins:
  - enable: true
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      secretRefs:
        - name: api-keys
      upstreamHeaderFields:
        - "X-Consumer-Username"
        - "X-Customer-ID"
        - "X-User-Tier"
```

---

## 行为细节

- 多个 `keySources` 按顺序检查，第一个找到非空值的来源用于认证
- 设置 `hideCredentials: true` 后，认证使用的来源（Header/Query/Cookie）会从请求中移除
- `anonymous` 模式下，未提供 Key 的请求也会通过，但会设置 `X-Anonymous-Consumer: true`
- Key 存储在 Kubernetes Secret 的 `keys.yaml` 字段中，格式为 YAML 数组

---

## 故障排除

### 问题 1：始终返回 401

**原因**：Secret 未正确配置或 key 格式不正确。

**解决方案**：
```bash
kubectl get secret api-keys -o yaml
# 确保 keys.yaml 字段存在且格式正确
```

### 问题 2：Key 匹配不上

**原因**：`keyField` 与 Secret 中的字段名不一致。

**解决方案**：确保 `keyField` 配置与 Secret 中 keys.yaml 的字段名匹配。

---

## 完整示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected-api
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: key-auth-plugin
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: key-auth-plugin
spec:
  requestPlugins:
    - enable: true
      type: KeyAuth
      config:
        keySources:
          - type: header
            name: "X-API-Key"
          - type: query
            name: api_key
        secretRefs:
          - name: api-keys
        hideCredentials: true
        realm: "Protected API"
---
apiVersion: v1
kind: Secret
metadata:
  name: api-keys
type: Opaque
stringData:
  keys.yaml: |
    - key: "production-key-001"
      username: "service-a"
    - key: "production-key-002"
      username: "service-b"
```

## 相关文档

- [Basic Auth](./basic-auth.md)
- [JWT Auth](./jwt-auth.md)
- [HMAC Auth](./hmac-auth.md)
