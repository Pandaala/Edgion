---
name: edgion-tls-features
description: EdgionTls CRD 完整 Schema：mTLS 双向认证、TLS 版本控制、密码套件、OCSP、会话管理。
---

# EdgionTls 资源

> API: `edgion.io/v1` | Scope: Namespaced

EdgionTls 是 Edgion 的扩展 TLS 配置资源，提供超越 Gateway 内置 `tls.certificateRefs` 的高级 TLS 功能。

## 完整 Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: my-tls
  namespace: default
  annotations:
    edgion.io/expose-client-cert: "true"          # 暴露客户端证书信息到请求头
spec:
  parentRefs:                                      # 挂载到 Gateway Listener
    - name: my-gateway
      sectionName: https

  hosts:                                           # 必填：适用的主机名列表
    - "api.example.com"
    - "*.example.com"

  secretRef:                                       # 必填：TLS 证书 Secret 引用
    name: my-cert-secret
    namespace: cert-ns                             # 跨命名空间需 ReferenceGrant

  clientAuth:                                      # mTLS 客户端认证配置
    mode: Mutual                                   # Terminate | Mutual | OptionalMutual
    caSecretRef:                                   # mode=Mutual/OptionalMutual 时必填
      name: ca-cert-secret
      namespace: cert-ns
    verifyDepth: 1                                 # 证书链验证深度（1-9，默认 1）
    allowedSans:                                   # SAN 白名单
      - "client.example.com"
    allowedCns:                                    # CN 白名单
      - "My Client"

  minTlsVersion: Tls12                             # 最低 TLS 版本

  ciphers:                                         # OpenSSL 格式密码套件列表
    - "ECDHE-ECDSA-AES256-GCM-SHA384"
    - "ECDHE-RSA-AES256-GCM-SHA384"

  extend:                                          # 扩展/实验性 TLS 配置
    preferServerCiphers: true                      # 优先使用服务端密码套件顺序

    ocspStapling:                                  # OCSP Stapling
      enabled: true
      refreshIntervalSeconds: 3600
      failOpen: false

    sessionTicket:                                 # Session Ticket
      enabled: true
      lifetimeSeconds: 7200
      rotationIntervalSeconds: 3600

    sessionCache:                                  # Session Cache
      enabled: true
      maxEntries: 10000
      ttlSeconds: 3600

    revocationCheck:                               # 证书吊销检查
      mode: ocsp                                   # off | ocsp | crl
      failOpen: false

    earlyData:                                     # TLS 1.3 0-RTT
      enabled: false
      rejectOnReplay: true
```

## spec 字段详解

### 基础字段

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `parentRefs` | `Vec<ParentReference>` | 否 | 挂载到 Gateway Listener |
| `hosts` | `Vec<String>` | 是 | 适用的主机名列表 |
| `secretRef` | `SecretObjectReference` | 是 | TLS 证书 Secret 引用 |
| `minTlsVersion` | `TlsVersion?` | 否 | 最低 TLS 版本 |
| `ciphers` | `Vec<String>?` | 否 | OpenSSL 格式密码套件 |

### TlsVersion 枚举

| 值 | 说明 |
|----|------|
| `Tls10` | TLS 1.0（不推荐） |
| `Tls11` | TLS 1.1（不推荐） |
| `Tls12` | TLS 1.2（推荐最低版本） |
| `Tls13` | TLS 1.3 |

### clientAuth — mTLS 配置

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `mode` | `ClientAuthMode` | `Terminate` | 认证模式 |
| `caSecretRef` | `SecretObjectReference?` | — | CA 证书 Secret（Mutual/OptionalMutual 必填） |
| `verifyDepth` | `u8` | `1` | 证书链验证深度（1-9） |
| `allowedSans` | `Vec<String>?` | — | Subject Alternative Name 白名单 |
| `allowedCns` | `Vec<String>?` | — | Common Name 白名单 |

**ClientAuthMode 枚举**:

| 值 | 说明 |
|----|------|
| `Terminate` | 单向 TLS：仅验证服务端证书（默认） |
| `Mutual` | 双向 TLS：必须提供有效客户端证书 |
| `OptionalMutual` | 可选双向 TLS：客户端证书可选，提供则验证 |

### extend — 扩展 TLS 配置

#### ocspStapling

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enabled` | `bool` | `false` | 启用 OCSP Stapling |
| `refreshIntervalSeconds` | `u64?` | — | OCSP 响应刷新间隔 |
| `failOpen` | `bool` | `false` | OCSP 不可用时是否放行 |

#### sessionTicket

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enabled` | `bool` | `false` | 启用 Session Ticket |
| `lifetimeSeconds` | `u64?` | — | Ticket 生命周期 |
| `rotationIntervalSeconds` | `u64?` | — | 密钥轮转间隔 |

#### sessionCache

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enabled` | `bool` | `false` | 启用 Session Cache |
| `maxEntries` | `u64?` | — | 最大缓存条目数 |
| `ttlSeconds` | `u64?` | — | 条目 TTL |

#### revocationCheck

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `mode` | `RevocationMode` | `Off` | `Off` / `Ocsp` / `Crl` |
| `failOpen` | `bool` | `false` | 吊销数据不可用时放行 |

#### earlyData (TLS 1.3 0-RTT)

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `enabled` | `bool` | `false` | 启用 0-RTT |
| `rejectOnReplay` | `bool` | `false` | 检测到重放时拒绝 |

## 与 Gateway TLS 的关系

| 功能 | Gateway `tls` | EdgionTls |
|------|--------------|-----------|
| 基础证书绑定 | ✅ certificateRefs | ✅ secretRef |
| mTLS 客户端认证 | ❌ | ✅ clientAuth |
| TLS 版本控制 | ❌ | ✅ minTlsVersion |
| 密码套件控制 | ❌ | ✅ ciphers |
| OCSP Stapling | ❌ | ✅ extend.ocspStapling |
| Session 管理 | ❌ | ✅ extend.sessionTicket/sessionCache |

使用 EdgionTls 时，设置 Gateway Listener 的 `tls.options.edgion.io/cert-provider: "edgion-tls"`。
