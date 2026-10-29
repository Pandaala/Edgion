# TLS 终结

## 下游 TLS 默认策略

`EdgionGatewayConfig.spec.downstreamTls` 为绑定到该 GatewayClass 配置的 TLS 终结监听器提供默认策略，
同时适用于原生 Gateway 证书和 EdgionTls 证书：

```yaml
spec:
  downstreamTls:
    minTlsVersion: TLS1_2
    ciphers: [ECDHE-RSA-AES128-GCM-SHA256, ECDHE-ECDSA-AES128-GCM-SHA256]
    ecdhCurves: [P-256, P-384]
```

| 字段 | 继承与覆盖规则 |
|---|---|
| `minTlsVersion` | EdgionTls 显式值优先，其次为此默认值，最后为后端默认值。局部 `TLS1_2` 可覆盖全局 `TLS1_3`；`TLS1_0`/`TLS1_1` 仍是已弃用的显式选择。 |
| `ciphers` | 优先级：EdgionTls 显式列表 → EGC 列表 → FIPS/后端默认值。仅当两处字段都缺省时才继承 FIPS/后端默认值。任一层的显式 `[]` 都表示使用后端默认套件，包括启用 FIPS 策略时。仅影响 TLS 1.2 及更早版本。 |
| `ecdhCurves` | 由 `X25519`、`P-256`、`P-384`、`P-521` 组成的有序列表，长度为 1–4 且不能重复。EdgionTls 和 listener 均没有曲线覆盖字段。 |

启用 FIPS 策略时，缺省曲线为 `P-256/P-384/P-521`；显式 NIST 子集保留其顺序，
包含 `X25519` 则拒绝握手。密码套件覆盖不会绕过曲线限制。未启用 FIPS 策略时，
缺省曲线采用后端默认值。空列表、重复名称和不支持的曲线均无效。

实际生效的密码套件配置无效时拒绝握手，不回退到更宽松的默认值；有效的局部密码套件
覆盖可屏蔽无效的继承列表。Controller 验证传输安全语法，Gateway 的 BoringSSL 在应用
实际策略时验证套件表达式。发布前被拒绝的更新保留此前已接收的配置代际。

新握手捕获一个 EGC 配置代际并在协议协商前应用策略；已建立连接保留协商结果。
物理端口保留启动时所属的 EGC 绑定，热更新不会重新绑定 GatewayClass 引用或转移共享监听器所有权。


使用标准 Gateway API 监听器配置，在 Gateway 上终结客户端 TLS。

## 概述

**TLS 终结**是指 Gateway 在监听器处解密客户端的 TLS 连接，检查明文请求（主机、路径、请求头），然后以明文或通过新 TLS 连接重新加密的方式转发到后端：

```
Client --[TLS]--> Gateway (decrypt) --[plaintext or re-encrypted TLS]--> Backend
```

只要 Gateway 需要对 HTTPS 端点进行七层路由（HTTPRoute / GRPCRoute），就需要 TLS 终结，因为路由决策依赖于解密后的请求。

这是标准 Gateway API 的方式：一个 `protocol: HTTPS` 的 Listener，配置 `tls.mode: Terminate` 和指向 `kubernetes.io/tls` Secret 的 `certificateRefs`。与之相对的是 **TLS Passthrough**（`tls.mode: Passthrough` + `TLSRoute`），Gateway 将加密字节直接转发到后端而不解密——详见 [TCP / TLS 监听器](../listeners/tcp.md)和 [TLSRoute 总览](../../../user-guide/tls-route/overview.md)。

如果需要 Gateway API 之外的 TLS 功能（mTLS、最低 TLS 版本、自定义密码套件），请使用 [EdgionTls 扩展](./edgion-tls.md)替代或补充标准监听器配置。

## 配置

### 方式 1：Gateway 监听器（标准 Gateway API）

一个完整可应用的 Gateway，包含一个终结 TLS 的 HTTPS 监听器：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      hostname: "example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: example-com-tls
```

关键字段：

| 字段 | 值 | 说明 |
|------|-----|------|
| `protocol` | `HTTPS` | 七层终结使用 `HTTPS`；`TLS` 用于原始 TLS 流的 Passthrough / 终结 |
| `tls.mode` | `Terminate` | 在 Gateway 解密，`Passthrough` 将加密字节转发到后端 |
| `tls.certificateRefs[].name` | Secret 名称 | 必须引用 Gateway 同命名空间的 `kubernetes.io/tls` Secret，跨命名空间访问需要 `ReferenceGrant` |

### 方式 2：EdgionTls 扩展

> **🔌 Edgion 扩展**
>
> `EdgionTls` 是 Edgion 自定义 CRD，提供标准 Gateway API 不支持的 TLS 功能——双向 TLS、最低 TLS 版本和 OpenSSL 格式密码套件列表。

需要这些额外功能时，在监听器上挂载 EdgionTls 资源替代（或补充）`certificateRefs`。schema 和所有参数详见 [EdgionTls 扩展](./edgion-tls.md)：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: example-tls
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - example.com
  secretRef:
    name: example-com-tls
    namespace: production
  # 最低接受的 TLS 版本（可选）。
  minTlsVersion: TLS1_2
```

## 证书管理

### 创建 TLS Secret

证书 Secret 必须是 `kubernetes.io/tls` 类型，并包含 `tls.crt` 和 `tls.key` 键。

```bash
# 直接从 PEM 文件创建（推荐）
kubectl create secret tls example-com-tls \
  --cert=path/to/tls.crt \
  --key=path/to/tls.key \
  -n production
```

或以 YAML 声明，数据字段使用 base64 编码的 PEM：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: example-com-tls
  namespace: production
type: kubernetes.io/tls
data:
  tls.crt: <base64-encoded-cert>
  tls.key: <base64-encoded-key>
```

⚠️ **切勿将真实私钥提交到 Git 或粘贴到文档中。** 上方的 `<base64-encoded-cert>` / `<base64-encoded-key>` 均为占位符。

### 证书链

如果客户端需要完整链来验证，请将叶证书后跟中间证书拼接到 `tls.crt`（叶证书在前）：

```text
-----BEGIN CERTIFICATE-----
(leaf / server certificate)
-----END CERTIFICATE-----
-----BEGIN CERTIFICATE-----
(intermediate CA certificate)
-----END CERTIFICATE-----
```

## 示例

### 示例 1：单域名证书

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      hostname: "example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: example-com-tls
```

### 示例 2：通配符证书

通配符证书仅覆盖一个子域名标签（RFC 6125 §6.4.3）：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    - name: https-wildcard
      port: 443
      protocol: HTTPS
      hostname: "*.example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: wildcard-example-com-tls
```

> **通配符深度说明**：`*.example.com` 覆盖 `api.example.com` 和 `app.example.com`，但**不覆盖**顶级域名 `example.com`，也**不覆盖** `deep.sub.example.com`。Edgion 使用 RFC 6125 单标签规则按 SNI 选择证书，因此超出该单标签范围的主机连接 TLS 握手会失败，除非额外配置了更具体的监听器 / 证书（例如 `*.sub.example.com` 或显式的 `example.com` 监听器）。请注意，HTTPRoute / TLSRoute 的主机名*路由*匹配遵循 Gateway API §4.3（多标签），只有证书选择遵循严格的 RFC 6125 单标签规则。

### 示例 3：同一 Gateway 上的多证书 / SNI

为不同域名使用不同证书时，每个 hostname 声明一个监听器，Gateway 根据客户端的 SNI 选择对应证书：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    # 顶级域名 + www 使用同一证书
    - name: https-apex
      port: 443
      protocol: HTTPS
      hostname: "example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: example-com-tls
    # 所有单标签子域名使用通配符证书
    - name: https-wildcard
      port: 443
      protocol: HTTPS
      hostname: "*.example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: wildcard-example-com-tls
    # 同一 Gateway 上的第二个不相关域名
    - name: https-other
      port: 443
      protocol: HTTPS
      hostname: "api.partner.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: partner-com-tls
```

每个监听器拥有独立证书，Gateway 根据客户端发送的 SNI 按连接选择证书。

## 相关文档

- [HTTPS 监听器](../listeners/https.md)
- [TCP / TLS 监听器](../listeners/tcp.md)
- [EdgionTls 扩展](./edgion-tls.md)
- [ACME 自动证书](./acme.md)
- [TLSRoute 总览](../../../user-guide/tls-route/overview.md)
