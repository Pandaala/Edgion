# JWE Decrypt 插件

## 概述

`JweDecrypt` 在请求阶段解密请求头中的 Compact JWE（当前支持 `alg=dir` + `enc=A256GCM`），
并将解密后的明文传递给上游服务。

适用场景：
- 客户端使用 JWE 传递加密身份载荷
- 网关侧统一解密，后端只消费明文或映射后的身份头
- 需要与 `KeyAuth` / `JwtAuth` 类似的标准认证失败响应

## 功能特点

- 复用公共认证能力：
  - `send_auth_error_response()` 统一错误响应
  - `set_claims_headers()` 做 payload 字段到头映射（含注入防护与大小限制）
- 支持严格模式（`strict`）和旁路模式
- 支持密钥延迟加载（首次请求加载 Secret）
- 支持 `payloadToHeaders` dot-notation 路径（如 `user.department`）
- 支持失败延迟（`authFailureDelayMs`）降低时序侧信道风险

## 配置参数（Phase 1）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `secretRef` | object | - | 引用包含 `secret` 字段的 K8s Secret |
| `keyManagementAlgorithm` | enum | `Dir` | 密钥管理算法（当前仅 `Dir`） |
| `contentEncryptionAlgorithm` | enum | `A256GCM` | 内容加密算法（当前仅 `A256GCM`） |
| `header` | string | `authorization` | 读取 JWE 的请求头 |
| `forwardHeader` | string | `authorization` | 写入解密明文的请求头 |
| `stripPrefix` | string | - | 提取 token 前剥离前缀（如 `Bearer `） |
| `strict` | bool | `true` | 缺少 token 时是否拒绝 |
| `hideCredentials` | bool | `false` | 是否移除原始凭证头 |
| `maxTokenSize` | integer | `8192` | token 最大长度（字节） |
| `allowedAlgorithms` | enum[] | - | `enc` 白名单 |
| `payloadToHeaders` | map | - | 解密 payload 字段映射到上游头 |
| `maxHeaderValueBytes` | integer | `4096` | 单个映射头 value 大小上限 |
| `maxTotalHeaderBytes` | integer | `16384` | 全部映射头总大小上限 |
| `storePayloadInCtx` | bool | `false` | 是否写入 ctx 变量 `jwe_payload` |
| `authFailureDelayMs` | integer | `0` | 失败延迟毫秒数 |

## 示例配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-test
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: JweDecrypt
      config:
        secretRef:
          name: jwe-secret
        keyManagementAlgorithm: Dir
        contentEncryptionAlgorithm: A256GCM
        header: authorization
        forwardHeader: x-decrypted-auth
        stripPrefix: "Bearer "
        strict: true
        hideCredentials: true
        allowedAlgorithms: [A256GCM]
        payloadToHeaders:
          uid: x-user-id
          user.department: x-user-dept
          permissions.admin: x-is-admin
```

对应 Secret：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwe-secret
  namespace: default
type: Opaque
data:
  secret: MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY= # 32-byte key 的 base64
```

## 错误语义

| 场景 | 状态码 | PluginLog |
|------|--------|-----------|
| 缺少 JWE 且 `strict=true` | `403` | `jwe:no-token` |
| JWE 格式错误 | `400` | `jwe:invalid-format` |
| JWE 缺少必要头（`alg`/`enc`） | `400` | `jwe:missing-header` |
| Secret/密钥不可用 | `403` | `jwe:no-key` |
| 密钥长度与算法不匹配 | `500` | `jwe:key-len-err` |
| 解密失败 | `403` | `jwe:decrypt-fail` |
| 算法不支持或不匹配 | `400` | `jwe:bad-alg` |
| 算法不在白名单 | `400` | `jwe:alg-denied` |
| token 超过大小限制 | `400` | `jwe:too-large` |

