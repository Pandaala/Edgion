# mTLS 配置

> **🔌 Edgion 扩展**
> 
> mTLS 配置通过 `EdgionTls` CRD 实现，这是 Edgion 的扩展功能，不属于标准 Gateway API 规范。

配置双向 TLS（Mutual TLS）认证。

## 概述

mTLS 要求客户端和服务器双方都验证对方的证书：

```
Client [cert] ←→ [verify] Gateway [cert] ←→ [verify] Client
```

## 配置方式

使用 EdgionTls 资源配置 mTLS：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-config
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Require  # 要求客户端证书
    caCertificateRefs:
      - name: client-ca
```

## 配置参考

### clientAuth

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| mode | string | | None/Request/Require |
| caCertificateRefs | array | | 客户端 CA 证书 |

### clientAuth.mode

| 模式 | 说明 |
|------|------|
| None | 不验证客户端证书（默认） |
| Request | 请求客户端证书但不强制 |
| Require | 强制要求客户端证书 |

## CA 证书 Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
type: Opaque
data:
  ca.crt: <base64-encoded-ca-cert>
```

## 示例

### 示例 1: 强制 mTLS

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: strict-mtls
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Require
    caCertificateRefs:
      - name: trusted-client-ca
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: mtls-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        certificateRefs:
          - group: edgion.io
            kind: EdgionTls
            name: strict-mtls
```

### 示例 2: 可选 mTLS

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: optional-mtls
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Request  # 请求但不强制
    caCertificateRefs:
      - name: trusted-client-ca
```

## 客户端配置

使用 curl 测试 mTLS：

```bash
curl --cert client.crt --key client.key \
     --cacert server-ca.crt \
     https://example.com/api
```

## 相关文档

- [TLS 终结](../gateway/tls/tls-termination.md)
- [后端 TLS](../../user-guide/http-route/backends/backend-tls.md)
