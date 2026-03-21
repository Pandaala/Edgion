---
name: tls-features
description: TLS 功能：EdgionTls（mTLS/版本/密码套件）、ACME 自动证书、BackendTLSPolicy 上游 TLS。
---

# 05 TLS 功能

> Edgion 的 TLS 体系：下游 TLS 终止、mTLS 双向认证、自动证书、上游 TLS。

## 文件清单

| 文件 | 主题 |
|------|------|
| [00-edgion-tls.md](00-edgion-tls.md) | EdgionTls CRD：mTLS、TLS 版本控制、密码套件、OCSP、会话管理 |
| [01-acme.md](01-acme.md) | EdgionAcme CRD：Let's Encrypt 自动证书签发与续期 |
| [02-backend-tls-policy.md](02-backend-tls-policy.md) | BackendTLSPolicy：上游后端 TLS/mTLS 配置 |

## TLS 架构概览

```
客户端 ──TLS──► Gateway ──TLS/Plain──► 后端
               │                      │
               ▼                      ▼
         下游 TLS                上游 TLS
         ├─ Gateway TLS         └─ BackendTLSPolicy
         │  (certificateRefs)
         └─ EdgionTls
            (mTLS, 版本, 密码套件)
```

| 方向 | 资源 | 用途 |
|------|------|------|
| 下游（客户端→网关） | Gateway `tls.certificateRefs` | 基础 TLS 证书 |
| 下游（客户端→网关） | EdgionTls | 扩展 TLS：mTLS、版本、密码套件 |
| 自动证书 | EdgionAcme | Let's Encrypt 自动签发和续期 |
| 上游（网关→后端） | BackendTLSPolicy | 后端 TLS 验证和 mTLS |
