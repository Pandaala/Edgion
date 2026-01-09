# TLS 终结

配置 Gateway 的 TLS 终结。

## 概述

TLS 终结是指 Gateway 解密客户端的 TLS 连接，然后以明文或重新加密的方式转发到后端：

```
Client → [TLS] → Gateway → [明文/TLS] → Backend
```

## 配置方式

### 方式 1: Gateway TLS 配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tls-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        mode: Terminate
        certificateRefs:
          - name: my-tls-secret
```

### 方式 2: EdgionTls 扩展

> **🔌 Edgion 扩展**
> 
> `EdgionTls` 是 Edgion 自定义 CRD，提供比标准 Gateway API 更丰富的 TLS 配置选项。

使用 EdgionTls 资源提供更多 TLS 选项：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: advanced-tls
spec:
  secretRef:
    name: my-tls-secret
  minVersion: TLSv1.2
  cipherSuites:
    - TLS_AES_128_GCM_SHA256
    - TLS_AES_256_GCM_SHA384
```

## 证书管理

### 创建 TLS Secret

```bash
# 从文件创建
kubectl create secret tls my-tls-secret \
  --cert=path/to/cert.pem \
  --key=path/to/key.pem

# 或使用 YAML
apiVersion: v1
kind: Secret
metadata:
  name: my-tls-secret
type: kubernetes.io/tls
data:
  tls.crt: <base64-encoded-cert>
  tls.key: <base64-encoded-key>
```

### 证书链

如果需要完整证书链，将中间证书附加到 tls.crt：

```
-----BEGIN CERTIFICATE-----
(服务器证书)
-----END CERTIFICATE-----
-----BEGIN CERTIFICATE-----
(中间证书)
-----END CERTIFICATE-----
```

## 示例

### 示例 1: 单域名证书

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: single-domain
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      hostname: "example.com"
      tls:
        certificateRefs:
          - name: example-com-tls
```

### 示例 2: 通配符证书

```yaml
listeners:
  - name: https
    port: 443
    protocol: HTTPS
    hostname: "*.example.com"
    tls:
      certificateRefs:
        - name: wildcard-example-com-tls
```

## 相关文档

- [HTTPS 监听器](../listeners/https.md)
- [EdgionTls 扩展](./edgion-tls.md)
- [mTLS 配置](../../infrastructure/mtls.md)
