# HTTPS 监听器

配置 HTTPS 协议监听器，实现 TLS 终结。

## 基本配置

```yaml
listeners:
  - name: https
    port: 443
    protocol: HTTPS
    tls:
      mode: Terminate
      certificateRefs:
        - name: tls-secret
```

## 配置参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| name | string | ✓ | 监听器名称 |
| port | int | ✓ | 监听端口 |
| protocol | string | ✓ | 协议（HTTPS） |
| tls | object | ✓ | TLS 配置 |
| hostname | string | | 主机名过滤 |

### TLS 配置

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| mode | string | | Terminate（默认）或 Passthrough |
| certificateRefs | array | ✓ | 证书 Secret 引用 |
| options | map | | TLS 选项 |

## TLS 模式

### Terminate - TLS 终结

Gateway 解密 TLS，转发明文到后端：

```yaml
tls:
  mode: Terminate
  certificateRefs:
    - name: tls-secret
```

### Passthrough - TLS 透传

Gateway 不解密，直接转发到后端：

```yaml
tls:
  mode: Passthrough
```

## 证书 Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: tls-secret
type: kubernetes.io/tls
data:
  tls.crt: <base64-encoded-cert>
  tls.key: <base64-encoded-key>
```

## 示例

### 示例 1: 基本 HTTPS

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: https-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        certificateRefs:
          - name: wildcard-tls
```

### 示例 2: 多域名证书

```yaml
listeners:
  - name: https-api
    port: 443
    protocol: HTTPS
    hostname: "api.example.com"
    tls:
      certificateRefs:
        - name: api-tls
  - name: https-web
    port: 443
    protocol: HTTPS
    hostname: "www.example.com"
    tls:
      certificateRefs:
        - name: web-tls
```

### 示例 3: HTTP 自动跳转 HTTPS

```yaml
listeners:
  - name: http
    port: 80
    protocol: HTTP
  - name: https
    port: 443
    protocol: HTTPS
    tls:
      certificateRefs:
        - name: tls-secret
```

配合 HTTPRoute 的 RequestRedirect 过滤器实现跳转。

## 相关文档

- [HTTP 监听器](./http.md)
- [TLS 终结](../tls/tls-termination.md)
- [EdgionTls 扩展](../tls/edgion-tls.md)
