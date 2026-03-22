---
name: backend-tls-policy-features
description: BackendTLSPolicy 完整 Schema：上游后端 TLS 验证和 mTLS。
---

# BackendTLSPolicy 资源

> API: `gateway.networking.k8s.io/v1alpha3` | Scope: Namespaced
> Gateway API v1.4.0 Experimental 资源

BackendTLSPolicy 定义网关到后端的 TLS 连接配置，包括 CA 验证和客户端证书（mTLS）。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha3
kind: BackendTLSPolicy
metadata:
  name: my-backend-tls
  namespace: default
spec:
  targetRefs:                                      # 目标 Service
    - group: ""
      kind: Service
      name: backend-service
      sectionName: https                           # 可选：指定 Service 端口名

  validation:
    hostname: "backend.internal.svc"               # 必填：TLS SNI 和证书验证主机名
    caCertificateRefs:                             # CA 证书引用
      - group: ""
        kind: Secret
        name: backend-ca-cert
    subjectAltNames:                               # SAN 白名单（可选）
      - type: Hostname
        hostname: "backend.internal.svc"
      - type: URI
        uri: "spiffe://cluster.local/ns/default/sa/backend"
    wellKnownCACertificates: System                # 使用系统 CA（可选）

  options:                                         # Edgion 扩展选项
    edgion.io/client-certificate-ref: "default/client-cert"  # 客户端证书（mTLS 到后端）
```

## spec 字段详解

### targetRefs

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `group` | `String` | 是 | 通常 `""` (core) |
| `kind` | `String` | 是 | 通常 `Service` |
| `name` | `String` | 是 | Service 名称 |
| `sectionName` | `String?` | 否 | Service 端口名 |

### validation

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `hostname` | `String` | 是 | TLS SNI 和证书验证的主机名 |
| `caCertificateRefs` | `Vec<CACertRef>?` | 否 | CA 证书引用列表 |
| `subjectAltNames` | `Vec<SubjectAltName>?` | 否 | SAN 白名单 |
| `wellKnownCACertificates` | `String?` | 否 | `System`：使用系统 CA 证书 |

**SubjectAltName**:

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | `String` | `Hostname` / `URI` |
| `hostname` | `String?` | type=Hostname 时必填 |
| `uri` | `String?` | type=URI 时必填 |

### options — Edgion 扩展

| Key | 值格式 | 说明 |
|-----|--------|------|
| `edgion.io/client-certificate-ref` | `ns/name` | 客户端证书 Secret（用于网关→后端 mTLS） |

## Status Schema

```yaml
status:
  ancestors:
    - ancestorRef:
        group: gateway.networking.k8s.io
        kind: Gateway
        name: my-gateway
        namespace: default
      controllerName: edgion.io/gateway-controller
      conditions:
        - type: Accepted
          status: "True"
```
