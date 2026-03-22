---
name: acme-features
description: EdgionAcme CRD 完整 Schema：Let's Encrypt 自动证书签发、续期、DNS-01/HTTP-01 挑战。
---

# EdgionAcme 资源

> API: `edgion.io/v1` | Scope: Namespaced

EdgionAcme 实现 ACME 协议自动证书管理（Let's Encrypt 等），支持 HTTP-01 和 DNS-01 挑战。

## 完整 Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: my-acme
  namespace: default
spec:
  server: "https://acme-v02.api.letsencrypt.org/directory"   # ACME 服务器
  email: "admin@example.com"                                  # 必填：联系邮箱
  domains:                                                     # 必填：证书域名列表
    - "example.com"
    - "*.example.com"                                          # 通配符域名需 DNS-01

  keyType: ecdsa-p256                                          # 证书密钥算法

  challenge:
    challengeType: http-01                                     # http-01 | dns-01
    http01:
      gatewayRef:                                              # HTTP-01 需要的 Gateway 引用
        name: my-gateway
        namespace: default
    # dns01:                                                   # DNS-01 配置
    #   provider: cloudflare                                   # cloudflare | alidns
    #   credentialRef:
    #     name: cloudflare-api-token
    #     namespace: default
    #   propagationTimeout: 120
    #   propagationCheckInterval: 5

  renewal:
    renewBeforeDays: 30                                        # 到期前多少天续期
    checkInterval: 86400                                       # 续期检查间隔（秒）
    failBackoff: 300                                           # 失败后退避时间（秒）

  # externalAccountBinding:                                    # EAB（可选）
  #   keyId: "kid123"
  #   hmacKey: "base64url-encoded-key"

  storage:
    secretName: "my-acme-cert"                                 # 证书存储 Secret 名称
    secretNamespace: "default"                                 # 证书 Secret 命名空间

  autoEdgionTls:
    enabled: true                                              # 自动创建 EdgionTls
    name: "acme-my-acme"                                       # EdgionTls 名称
    parentRefs:                                                # EdgionTls 的 parentRefs
      - name: my-gateway
        sectionName: https
```

## spec 字段详解

### 基础字段

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `server` | `String` | Let's Encrypt 生产 | ACME 目录 URL |
| `email` | `String` | **必填** | ACME 账户注册邮箱 |
| `domains` | `Vec<String>` | **必填** | 证书域名列表 |
| `keyType` | `AcmeKeyType` | `ecdsa-p256` | 密钥算法 |

**AcmeKeyType 枚举**:

| 值 | 说明 |
|----|------|
| `ecdsa-p256` | ECDSA P-256（默认，推荐） |
| `ecdsa-p384` | ECDSA P-384 |

### challenge — 挑战配置

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `challengeType` | `String` | `http-01` | `http-01` / `dns-01` |

**HTTP-01**:

| 字段 | 类型 | 说明 |
|------|------|------|
| `http01.gatewayRef` | `ParentReference` | 用于响应 ACME 挑战的 Gateway |

**DNS-01**:

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `dns01.provider` | `String` | — | DNS 提供商：`cloudflare` / `alidns` |
| `dns01.credentialRef` | `SecretObjectReference` | — | API 凭证 Secret |
| `dns01.propagationTimeout` | `u64` | `120` | DNS 传播超时（秒） |
| `dns01.propagationCheckInterval` | `u64` | `5` | 传播检查间隔（秒） |

### renewal — 续期配置

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `renewBeforeDays` | `u32` | `30` | 到期前天数触发续期 |
| `checkInterval` | `u64` | `86400` | 续期检查间隔（秒，默认 24h） |
| `failBackoff` | `u64` | `300` | 失败退避时间（秒，默认 5min） |

### storage — 证书存储

| 字段 | 类型 | 说明 |
|------|------|------|
| `secretName` | `String` | 证书存储的 K8s Secret 名称 |
| `secretNamespace` | `String?` | Secret 命名空间（默认同 EdgionAcme） |

### autoEdgionTls — 自动 EdgionTls

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enabled` | `bool` | `true` | 自动创建/更新 EdgionTls |
| `name` | `String?` | `acme-{name}` | EdgionTls 资源名称 |
| `parentRefs` | `Vec<ParentReference>?` | — | EdgionTls 的 Gateway 挂载点 |

## Status Schema

```yaml
status:
  phase: Ready                                     # Pending | Issuing | Ready | Renewing | Failed
  certificateSerial: "1234567890"
  certificateNotAfter: "2026-06-20T00:00:00Z"      # 到期时间
  lastRenewalTime: "2026-03-20T12:00:00Z"
  lastFailureReason: ""
  lastFailureTime: ""
  secretName: "my-acme-cert"
  edgionTlsName: "acme-my-acme"
  accountUri: "https://acme-v02.api.letsencrypt.org/acme/acct/123"
```

**AcmeCertPhase 枚举**:

| Phase | 说明 |
|-------|------|
| `Pending` | 等待首次签发 |
| `Issuing` | ACME 签发中 |
| `Ready` | 证书有效 |
| `Renewing` | 续期中 |
| `Failed` | 上次操作失败 |

## 运行约束

- ACME 服务仅在 **Leader** Controller 上运行
- HTTP-01 挑战需要 Gateway 的 80 端口可达
- 通配符域名必须使用 DNS-01 挑战
