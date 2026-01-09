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
| validation.hostname | string | ✓ | 验证的主机名 |

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
