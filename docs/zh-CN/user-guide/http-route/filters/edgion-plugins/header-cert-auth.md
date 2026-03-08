# Header Cert Auth 插件

> **🔌 Edgion 扩展**
> 
> HeaderCertAuth 是 `EdgionPlugins` CRD 提供的客户端证书认证插件，不属于标准 Gateway API。

## 概述

Header Cert Auth 通过验证客户端证书来实现身份认证。支持两种模式：
- **Header 模式**：从 HTTP Header 中读取客户端证书（适用于前端代理已完成 TLS 终结的场景）
- **mTLS 模式**：从 mTLS 连接上下文中读取证书（直接 TLS 连接场景）

## 快速开始

### 创建 CA 证书 Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: client-ca-cert
type: Opaque
data:
  ca.crt: <base64-encoded-ca-certificate>
```

### 配置插件

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cert-auth-plugin
spec:
  requestPlugins:
    - enable: true
      type: HeaderCertAuth
      config:
        mode: Header
        certificateHeaderName: X-Client-Cert
        caSecretRefs:
          - name: client-ca-cert
```

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `mode` | String | ❌ | `Header` | 证书来源模式：`Header` / `Mtls` |
| `certificateHeaderName` | String | ❌ | `"X-Client-Cert"` | Header 模式下包含证书的 header 名 |
| `certificateHeaderFormat` | String | ❌ | `Base64Encoded` | 证书编码格式：`Base64Encoded` / `UrlEncoded` |
| `hideCredentials` | Boolean | ❌ | `true` | 是否移除证书 header |
| `caSecretRefs` | Array | ✅* | `[]` | CA 证书 Secret 引用 |
| `verifyDepth` | Integer | ❌ | `1` | 证书链验证深度 |
| `skipConsumerLookup` | Boolean | ❌ | `false` | 是否跳过消费者身份查找 |
| `consumerBy` | String | ❌ | `SanOrCn` | 消费者身份提取方式：`SanOrCn` / `San` / `Cn` |
| `allowAnonymous` | Boolean | ❌ | `false` | 是否允许无证书访问 |
| `errorStatus` | Integer | ❌ | `401` | 验证失败返回的状态码 |
| `errorMessage` | String | ❌ | `"TLS certificate failed verification"` | 验证失败消息 |
| `authFailureDelayMs` | Integer | ❌ | `0` | 认证失败延迟毫秒数 |

\* Header 模式下必填。

---

## 常见配置场景

### 场景 1：Header 模式（前端代理 + 证书传递）

```yaml
requestPlugins:
  - enable: true
    type: HeaderCertAuth
    config:
      mode: Header
      certificateHeaderName: X-Client-Cert
      certificateHeaderFormat: urlEncoded
      caSecretRefs:
        - name: client-ca-cert
      verifyDepth: 2
      hideCredentials: true
```

### 场景 2：mTLS 模式

```yaml
requestPlugins:
  - enable: true
    type: HeaderCertAuth
    config:
      mode: Mtls
      consumerBy: sanOrCn
```

### 场景 3：允许匿名访问

```yaml
requestPlugins:
  - enable: true
    type: HeaderCertAuth
    config:
      mode: Header
      caSecretRefs:
        - name: client-ca-cert
      allowAnonymous: true
```

---

## 行为细节

- **Header 模式**：从指定 Header 读取 PEM 格式证书，使用 `caSecretRefs` 中的 CA 证书进行验证
- **mTLS 模式**：从 TLS 握手上下文中直接获取已验证的客户端证书
- **消费者身份**：根据 `consumerBy` 配置从证书的 SAN 或 CN 提取身份信息，设置为 `X-Consumer-Username`
- 设置 `hideCredentials: true` 后，证书 Header 不会传递给上游

---

## 故障排除

### 问题 1：证书验证失败

**原因**：CA 证书不匹配或证书链不完整。

**解决方案**：
```bash
# 验证证书链
openssl verify -CAfile ca.crt client.crt
```

### 问题 2：Header 中的证书解析失败

**原因**：证书编码格式与 `certificateHeaderFormat` 不匹配。

**解决方案**：确保前端代理传递证书时使用与配置一致的编码格式。

---

## 相关文档

- [mTLS 配置](../../../../ops-guide/infrastructure/mtls.md)
- [TLS 终结](../../../../ops-guide/gateway/tls/tls-termination.md)
- [Basic Auth](./basic-auth.md)
