# HMAC Auth 插件

> **🔌 Edgion 扩展**
> 
> HmacAuth 是 `EdgionPlugins` CRD 提供的 HMAC 签名认证插件，不属于标准 Gateway API。

## 概述

HMAC Auth 基于 HTTP Signature 规范，通过 HMAC 算法验证请求签名，确保请求的完整性和身份真实性。支持 hmac-sha256、hmac-sha384、hmac-sha512 算法。

**工作原理**：
1. 客户端使用共享密钥对请求的指定部分（header、path 等）计算 HMAC 签名
2. 将签名信息放入 `Authorization` 或 `Signature` header
3. 插件使用相同的密钥重新计算签名并与请求中的签名比对
4. 签名匹配且在时间窗口内：允许访问
5. 签名不匹配或过期：返回 401

## 快速开始

### 创建凭证 Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: hmac-credentials
type: Opaque
stringData:
  username: "service-a"
  secret: "a-very-long-secret-key-at-least-32-chars!"
```

### 配置插件

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: hmac-auth-plugin
spec:
  requestPlugins:
    - enable: true
      type: HmacAuth
      config:
        secretRefs:
          - name: hmac-credentials
        algorithms:
          - hmac-sha256
        clockSkew: 300
```

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `secretRefs` | Array | ✅* | 无 | Kubernetes Secret 引用列表 |
| `algorithms` | Array | ❌ | `["hmac-sha256","hmac-sha384","hmac-sha512"]` | 允许的签名算法 |
| `clockSkew` | Integer | ❌ | `300` | 允许的时钟偏移秒数 |
| `enforceHeaders` | Array | ❌ | 无 | 签名中必须包含的 header 列表 |
| `validateRequestBody` | Boolean | ❌ | `false` | 是否验证请求体的 Digest |
| `hideCredentials` | Boolean | ❌ | `false` | 是否移除认证相关 header |
| `anonymous` | String | ❌ | 无 | 匿名用户名 |
| `realm` | String | ❌ | `"edgion"` | 认证域 |
| `authFailureDelayMs` | Integer | ❌ | `0` | 认证失败延迟响应毫秒数 |
| `minSecretLength` | Integer | ❌ | `32` | 最小密钥长度 |
| `secretField` | String | ❌ | `"secret"` | Secret 中密钥的字段名 |
| `usernameField` | String | ❌ | `"username"` | Secret 中用户名的字段名 |
| `upstreamHeaderFields` | Array | ❌ | `[]` | 传递给上游的额外 header |

\* 未设置 `anonymous` 时必填。

---

## 常见配置场景

### 场景 1：基础 HMAC 认证

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretRefs:
        - name: hmac-credentials
      algorithms:
        - hmac-sha256
      hideCredentials: true
```

### 场景 2：强制签名特定 Header

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretRefs:
        - name: hmac-credentials
      enforceHeaders:
        - "@request-target"
        - host
        - date
        - content-type
      validateRequestBody: true
```

### 场景 3：多用户认证

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretRefs:
        - name: user-a-credentials
        - name: user-b-credentials
      algorithms:
        - hmac-sha256
        - hmac-sha512
      clockSkew: 600
      upstreamHeaderFields:
        - X-Consumer-Username
```

---

## 签名格式

客户端请求需包含以下 Header：

```
Authorization: Signature keyId="service-a",algorithm="hmac-sha256",headers="@request-target host date",signature="base64-encoded-signature"
Date: Mon, 08 Mar 2026 10:00:00 GMT
```

### 签名计算步骤

1. 构建签名字符串：按 `headers` 参数指定的顺序拼接各 header 的值
2. 使用 HMAC 算法和共享密钥计算签名
3. Base64 编码签名结果

---

## 故障排除

### 问题 1：签名验证失败

**原因**：时钟偏移过大或签名计算错误。

**解决方案**：
- 确保客户端和服务端时间同步
- 增大 `clockSkew` 值
- 检查签名字符串的构造是否正确

### 问题 2：密钥长度不足

**原因**：Secret 中的密钥短于 `minSecretLength`。

**解决方案**：使用至少 32 字符的密钥，或调整 `minSecretLength`。

---

## 相关文档

- [Basic Auth](./basic-auth.md)
- [Key Auth](./key-auth.md)
- [JWT Auth](./jwt-auth.md)
