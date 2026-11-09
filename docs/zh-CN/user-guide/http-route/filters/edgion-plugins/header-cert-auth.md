# Header Cert Auth 插件

> **🔌 Edgion 扩展**
> 
> HeaderCertAuth 是 `EdgionPlugins` CRD 提供的客户端证书认证插件，不属于标准 Gateway API。

## 概述

Header Cert Auth 通过验证客户端证书来实现身份认证。支持两种模式：
- **Header 模式**（`mode: Header`）：从 HTTP Header 中读取客户端证书（适用于前端代理已完成 TLS 终结的场景）
- **Connection 模式**（`mode: Connection`）：复用 mTLS 连接上下文中已验证的客户端证书（直接 TLS 连接场景）

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
    - type: HeaderCertAuth
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
| `mode` | Enum | ❌ | `Header` | 证书来源模式：`Header` / `Connection` |
| `certificateHeaderName` | String | ❌ | `"X-Client-Cert"` | Header 模式下包含证书的 header 名 |
| `certificateHeaderFormat` | Enum | ❌ | `base64Encoded` | 证书编码格式（Header 模式）：`base64Encoded`（不含分隔行的 base64 PEM 正文）/ `urlEncoded`（URL 编码的完整 PEM） |
| `hideCredentials` | Boolean | ❌ | `true` | 转发上游前是否移除证书 header |
| `caSecretRefs` | Array | ✅* | `[]` | 用于验证客户端证书的 CA Secret 引用。所引用 Secret 中的所有证书都作为信任锚加载；若叶证书由中间 CA 签发，请同时包含中间 CA。Connection 模式不需要 |
| `skipConsumerLookup` | Boolean | ❌ | `false` | 是否跳过消费者映射，直接透传证书 DN/SAN |
| `consumerBy` | Enum | ❌ | `sanOrCn` | 消费者身份提取方式：`sanOrCn`（首个 SAN，缺失时回退到 CN）/ `cn`（仅 CN）/ `fingerprint`（证书 SHA-256 指纹） |
| `upstreamHeaders` | Object | ❌ | 见子表 | 向上游传递身份信息的请求头名称 |
| `allowAnonymous` | Boolean | ❌ | `false` | 认证失败时是否以匿名身份放行 |
| `errorStatus` | Integer | ❌ | `401` | 验证失败返回的状态码（必须在 400–599 之间） |
| `errorMessage` | String | ❌ | `"TLS certificate failed verification"` | 验证失败消息 |
| `authFailureDelayMs` | Integer | ❌ | `0` | 返回认证失败响应前的固定延迟（毫秒） |

\* Header 模式下必填。

### `upstreamHeaders` 子字段

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `consumerHeader` | String | ❌ | `"X-Consumer-Username"` | 解析出的消费者身份 |
| `dnHeader` | String | ❌ | `"X-Client-Cert-Dn"` | 证书主题 DN |
| `sanHeader` | String | ❌ | `"X-Client-Cert-San"` | 首个 SAN 值 |
| `fingerprintHeader` | String | ❌ | `"X-Client-Cert-Fingerprint"` | 证书 SHA-256 指纹 |

---

## 常见配置场景

### 场景 1：Header 模式（前端代理 + 证书传递）

```yaml
requestPlugins:
  - type: HeaderCertAuth
    config:
      mode: Header
      certificateHeaderName: X-Client-Cert
      certificateHeaderFormat: urlEncoded
      caSecretRefs:
        - name: client-ca-cert
      hideCredentials: true
```

### 场景 2：Connection（mTLS）模式

复用 TLS 握手阶段已验证的证书，无需配置 `caSecretRefs`。

```yaml
requestPlugins:
  - type: HeaderCertAuth
    config:
      mode: Connection
      consumerBy: sanOrCn
```

### 场景 3：允许匿名访问

```yaml
requestPlugins:
  - type: HeaderCertAuth
    config:
      mode: Header
      caSecretRefs:
        - name: client-ca-cert
      allowAnonymous: true
```

---

## 行为细节

- **Header 模式**：从指定 Header 读取 PEM 格式证书，使用 `caSecretRefs` 中的 CA 证书进行验证
- **Connection 模式**：从 mTLS 握手上下文中直接获取已验证的客户端证书
- **消费者身份**：根据 `consumerBy` 从证书的 SAN、CN 或指纹提取身份信息，写入 `upstreamHeaders.consumerHeader`（默认 `X-Consumer-Username`）
- 设置 `hideCredentials: true` 后，证书 Header 不会传递给上游

### 证书身份请求头的归属

插件执行时，会先清理当前启用的输出目标，再写入身份值：
`skipConsumerLookup: true` 时为 `dnHeader`、`sanHeader` 和 `fingerprintHeader`，
否则为 `consumerHeader` 和 `fingerprintHeader`。清理会移除同名的所有重复输入。
输出名称互不相同时，证书没有 SAN 会使 SAN 目标头保持不存在；不是合法 HTTP 请求头文本的
DN/SAN 和指纹值（例如包含控制字符）会被省略，不会保留客户端原值；这样的消费者身份视为无效
（`allowAnonymous: true` 时走匿名放行）。
匿名放行也先清理这些目标，再写入匿名消费者标记和身份。

请求头删除或写入失败（通常是配置问题，例如网关无法写入的头名）按认证失败处理：在
`authFailureDelayMs` 之后返回配置的 `errorStatus`，不转发请求，即使配置了 `allowAnonymous: true`。
访问日志记录 `certificate header mutation failed` 和诊断码 `AuthHeaderMutationRejected`。
认证失败仍采用原有的拒绝或匿名策略。禁用或条件跳过的插件不执行清理；未启用的输出名
以及无关请求头不属于保留名称。

后端需要分别读取各字段时，请配置不同的输出名称。同名输出仍按原顺序写入
（DN、首个 SAN、指纹；或消费者身份、指纹）。Header 模式先读取证书，再清理输出，
因此输入与输出可以同名；但 `hideCredentials: true` 仍会在最后移除该名称。
后续插件仍可按执行顺序修改这些请求头。

使用 EdgionTls 的 Connection 模式需要在 EdgionTls 上设置
`edgion.io/expose-client-cert: "true"`。Gateway 的 `frontendValidation` mTLS
会自动暴露证书元数据。

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

## 安全最佳实践

### ✅ 推荐做法

- 提供可信的 `caSecretRefs` CA 包并保持验证开启；若叶证书由中间 CA 签发，请同时包含中间 CA。
- 在 Header 模式下，确保只有可信的 TLS 终结代理才能设置 `certificateHeaderName`；对不可信入站流量剥离该头，防止客户端伪造证书。
- 对于已知客户端集合，结合 `consumerBy` 和上游授权进行访问限制。
- 保持 `hideCredentials: true`（默认值），除非后端本身需要证书。
- 当网关直接终结 TLS 时，优先使用 Connection（mTLS）模式——证书在握手阶段完成验证，无法通过伪造请求头绕过。

### ❌ 需要避免

- 信任来自 TLS 终结代理以外的 `certificateHeaderName`（请求头伪造）。
- 在必须要求客户端证书的路由上启用 `allowAnonymous: true`。
- 在后端不需要证书时，将证书请求头转发至上游（`hideCredentials: false`）。

---

## 相关文档

- [mTLS 配置](../../../../ops-guide/infrastructure/mtls.md)
- [TLS 终结](../../../../ops-guide/gateway/tls/tls-termination.md)
- [Basic Auth](./basic-auth.md)
- [JWT Auth](./jwt-auth.md)
