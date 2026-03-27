# 后端 TLS

配置 Gateway 到后端服务的 TLS 连接。

## 概述

使用 BackendTLSPolicy 配置后端 TLS：

```
Client → [TLS] → Gateway → [TLS] → Backend
```

## 配置 BackendTLSPolicy

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: BackendTLSPolicy
metadata:
  name: backend-tls
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: backend-ca
    hostname: backend.internal
```

## 配置参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| targetRefs | array | ✓ | 目标 Service 列表 |
| validation.caCertificateRefs | array | ✓ | CA 证书 Secret |
| validation.hostname | string | ✓ | 用于 SNI 和证书验证的主机名 |
| validation.subjectAltNames | array | | 额外的 SAN 验证规则 |
| validation.subjectAltNames[].type | string | ✓ | `Hostname` 或 `URI` |
| validation.subjectAltNames[].hostname | string | | DNS 名称（type=Hostname 时），支持通配符（`*.example.com`） |
| validation.subjectAltNames[].uri | string | | 完整 URI（type=URI 时），如 SPIFFE ID |

## CA 证书 Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: backend-ca
type: kubernetes.io/tls
data:
  ca.crt: <base64-encoded-ca-cert>
```

## Subject Alternative Name 验证

配置 `subjectAltNames` 后，Gateway 在 TLS 握手时验证后端证书的 SAN。不匹配则拒绝连接。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: backend-san-check
spec:
  targetRefs:
    - kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - kind: Secret
        name: backend-ca
    hostname: backend.internal
    subjectAltNames:
      - type: Hostname
        hostname: backend.internal
      - type: URI
        uri: spiffe://cluster.local/ns/default/sa/backend
```

匹配规则：
- **Hostname**：精确匹配或通配符（`*.example.com` 匹配单级子域名）
- **URI**：精确匹配（如 SPIFFE ID）
- 任一条目匹配即通过
- 不配置 `subjectAltNames` 时保持默认行为（仅 hostname 验证）

## 示例

### 示例 1: 内部服务 mTLS

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: BackendTLSPolicy
metadata:
  name: internal-mtls
spec:
  targetRefs:
    - kind: Service
      name: internal-api
  validation:
    caCertificateRefs:
      - kind: Secret
        name: internal-ca
    hostname: internal-api.svc.cluster.local
```

## 相关文档

- [Service 引用](./service-ref.md)
- [mTLS 配置](../../../ops-guide/infrastructure/mtls.md)
