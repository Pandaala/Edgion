# JWE Decrypt 插件

> **🔌 Edgion 扩展**
>
> JweDecrypt 是 `EdgionPlugins` CRD 提供的 JWE 解密认证插件，不属于标准 Gateway API。

## 概述

`JweDecrypt` 在请求阶段解密请求头中的 Compact JWE（当前支持 `alg=dir` + `enc=A256GCM`），并将解密后的明文传递给上游服务。

适用场景：
- 客户端使用 JWE 传递加密身份载荷
- 网关侧统一解密，后端只消费明文或映射后的身份头
- 需要与 `KeyAuth` / `JwtAuth` 类似的标准认证失败响应

## 功能特点

- 复用公共认证能力：
  - `send_auth_error_response()` 统一错误响应
  - `set_claims_headers()` 做 payload 字段到头映射（含注入防护与大小限制）
- 支持严格模式（`strict`）和旁路模式
- 每一代插件运行时在构造时准备一个不可变的 AES-GCM cipher；Secret 更新由 Controller 重新解析，并通过发布替换代生效
- protected header 必须是成员名唯一的 JSON 对象，并拒绝包含 `zip` 或 `crit` 的 protected header；未知的非 critical 参数仍可接受，并作为原始 protected-header AAD 的一部分参与认证
- 支持 `payloadToHeaders` dot-notation 路径（如 `user.department`）
- 支持失败延迟（`authFailureDelayMs`）降低时序侧信道风险
- 支持 `base64Secret` 对双重 base64 编码的密钥做额外解码

## 快速开始

创建解密密钥 Secret，然后关联到插件和 HTTPRoute：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwe-key-secret
  namespace: production
type: Opaque
data:
  secret: MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=  # 32 字节密钥的 base64
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-plugin
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        strict: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: jwe-decrypt-plugin
      backendRefs:
        - name: api-service
          port: 8080
```

> 只指定 `secretRef` 时，其他所有字段使用默认值：读取 `authorization` 头，将解密后的 payload 写回同一个 `authorization` 头，启用严格模式，不剥离前缀。

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `secretRef` | Object | **必填** | 引用包含 `secret` 字段的 K8s Secret |
| `keyManagementAlgorithm` | Enum | `Dir` | 密钥管理算法（当前仅 `Dir`） |
| `contentEncryptionAlgorithm` | Enum | `A256GCM` | 内容加密算法（当前仅 `A256GCM`） |
| `header` | String | `authorization` | 读取 JWE token 的请求头 |
| `forwardHeader` | String | `authorization` | 写入解密明文的请求头（上游收到此头） |
| `stripPrefix` | String | 无 | 提取 token 前剥离的前缀（如 `Bearer `） |
| `strict` | Boolean | `true` | 无 token 时拒绝请求；设为 `false` 允许匿名放行 |
| `hideCredentials` | Boolean | `false` | 代理到上游前是否移除原始凭证头 |
| `base64Secret` | Boolean | `false` | 对 Secret 值额外进行一次 base64 解码（用于双重编码的密钥） |
| `maxTokenSize` | Integer | `8192` | token 最大长度（字节） |
| `allowedAlgorithms` | Array\<Enum\> | 无 | 内容加密算法白名单；若设置则必须包含 `contentEncryptionAlgorithm` |
| `payloadToHeaders` | Object | 无 | 将解密后的 JSON payload 字段映射到上游请求头（支持 dot-path，如 `user.department`） |
| `maxHeaderValueBytes` | Integer | `4096` | 每个映射头值的最大大小 |
| `maxTotalHeaderBytes` | Integer | `16384` | 所有映射头的总大小上限 |
| `storePayloadInCtx` | Boolean | `false` | 将解密后的 payload 存入上下文变量 `jwe_payload`（供下游插件读取；已从访问日志中排除） |
| `authFailureDelayMs` | Integer | `0` | 认证失败响应前的延迟毫秒数（降低时序侧信道风险） |

## 常见配置场景

### 1. 解密后转发到不同头

使用专用的 `x-decrypted-auth` 头传递给上游，而不是覆盖 `authorization`。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-forward
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        header: authorization
        forwardHeader: x-decrypted-auth
        stripPrefix: "Bearer "
        hideCredentials: true
```

### 2. 将 payload 字段映射到上游头

将解密后的 JSON payload 中的特定字段解包并注入为独立的上游头。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-claims
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        stripPrefix: "Bearer "
        hideCredentials: true
        payloadToHeaders:
          uid: x-user-id
          user.department: x-user-dept
          permissions.admin: x-is-admin
        maxHeaderValueBytes: 4096
        maxTotalHeaderBytes: 16384
```

### 3. 宽松模式 + 时序保护

允许无 JWE token 的请求（匿名放行），并在认证失败时添加 200ms 延迟以降低基于时序的枚举攻击风险。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-lenient
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        strict: false
        authFailureDelayMs: 200
```

### 4. 将 payload 存入上下文供下游插件使用

通过 `jwe_payload` 上下文变量将完整的解密 payload 提供给下游插件（如 DSL 或 CtxSet），不在访问日志中暴露。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-ctx
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        storePayloadInCtx: true
        strict: true
```

## 密钥准备与 Protected Header

Controller 在发布插件配置前解析引用的 Secret。Gateway 只在构造运行时插件代时消费 Controller 已解析的凭据，并为这一代准备一个不可变 cipher 或 fail-closed 不可用状态；请求路径不会重新加载 Secret，也不会回退读取 Gateway 本地 Secret。Secret 更新后，正常的 Controller 重新解析和 requeue 流程会发布替换的插件代；已经开始的请求继续使用其捕获的插件代。

当前 Compact JWE 实现不支持压缩或 critical extension。Protected Header Parameter 中只要存在名为 `zip` 或 `crit` 的字段，无论其值为何，都会被拒绝。其他未标记为 critical 的未知参数仍可接受。认证 AAD 始终使用原始编码的 protected segment，而不是重新序列化后的 JSON。这些约束只描述当前支持的 profile，不代表完整实现 RFC 7516。

## 错误语义

Operator-facing 配置错误通过 `Accepted` 报告。引用的 Secret 不存在、缺少 `data.secret`、在 `base64Secret` 模式下包含无效的内层 Base64，或没有解码成 A256GCM 所需的 32 字节密钥时，通过 `ResolvedRefs` 报告。不可用的凭据材料仍会构造 fail-closed `JweDecrypt` 运行时，而不是把资源封装成通用无效插件；替换代不会继续保留上一把密钥。

凭据缺失、密钥材料不可用和密文认证失败使用相同的通用 HTTP `401` 认证响应、Bearer challenge 和配置的 `authFailureDelayMs`。客户端不会收到编码细节、密钥长度或 Secret 标识。缺少 token 时，`strict=false` 仍会旁路插件；`strict=true` 返回通用的 401 必需-token 响应。

| 场景 | 状态码 | 客户端响应体 | PluginLog |
|------|--------|--------------|-----------|
| 缺少 JWE 且 `strict=true` | `401` | `401 JWE token required` | `no token` |
| JWE 格式错误，包括 protected header 不是对象、含重复成员、含 `zip` 或 `crit` | `400` | `400 Invalid JWE format` | `invalid format` |
| JWE 缺少必要头（`alg`/`enc`） | `400` | `400 JWE missing required headers` | `missing header` |
| 引用的 Secret/密钥不可用或无法解码 | `401` | `401 JWE decryption failed` | `err: key load fail` |
| 已解析密钥长度不符合 A256GCM | `401` | `401 JWE decryption failed` | `err: key len err` |
| 解密失败 | `401` | `401 JWE decryption failed` | `err: decrypt fail` |
| 算法不支持或不匹配 | `400` | `400 Unsupported encryption algorithm` | `bad alg` |
| 算法不在白名单 | `400` | `400 Algorithm not allowed` | `alg denied` |
| token 超过大小限制 | `400` | `400 JWE token too large` | `token too large` |

## 相关文档

- [JWE Decrypt — HTTPRoute 过滤器视角](../http-route/filters/edgion-plugins/jwe-decrypt.md) - 在 HTTPRoute 过滤器中使用 JweDecrypt
- [JwtAuth 插件](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT 令牌认证
- [OpenID Connect 插件](openid-connect.md) - OIDC/OAuth2 认证
- [ForwardAuth 插件](forward-auth.md) - 外部鉴权插件
- [CtxSet 插件](ctx-setter.md) - 上下文变量插件；可读取 `storePayloadInCtx` 写入的 `jwe_payload`
