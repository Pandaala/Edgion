# EdgionTls

> **🔌 Edgion 扩展**
>
> `EdgionTls` 是 Edgion 自定义 CRD（`edgion.io/v1`），提供 Gateway API 之外的 TLS 功能：双向 TLS（mTLS）、最低 TLS 版本和 OpenSSL 格式密码套件列表。不属于标准 Gateway API 规范。

EdgionTls 将服务端证书绑定到一个或多个 Gateway 监听器，并控制 TLS 握手行为。当标准 Gateway API 的 `tls.certificateRefs` 无法满足需求时使用——例如需要客户端证书验证、固定最低 TLS 版本或限制密码套件。

## 概述

EdgionTls 通过 `parentRefs` 附加到 Gateway 监听器，并按 `hosts` 列表中的 SNI 匹配传入连接。匹配时，Gateway 使用 `secretRef` 中的证书并应用已配置的握手选项：

```
Client --[TLS + SNI]--> Gateway listener
                          └─ EdgionTls (hosts match) -> serve secretRef cert,
                             enforce clientAuth / minTlsVersion / ciphers
```

要让监听器使用 EdgionTls 解析证书，需在 Gateway 的监听器选项中设置 `edgion.io/cert-provider: "edgion-tls"`。详见[在监听器上选择 EdgionTls](#在监听器上选择-edgiontls)。

如需自动签发和续期证书，可将 EdgionTls 与 [EdgionAcme](./acme.md) 配合使用——ACME 可自动创建和更新 EdgionTls。

## 快速开始

### 最简单的 HTTPS 配置

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
```

### 创建证书 Secret

```bash
kubectl create secret tls example-com-tls \
  --cert=path/to/tls.crt \
  --key=path/to/tls.key \
  -n production
```

Secret 必须是 `kubernetes.io/tls` 类型，包含 `tls.crt` 和 `tls.key` 键。⚠️ **切勿将真实私钥提交到版本控制或文档中。**

### 验证

```bash
kubectl get edgiontls -n production
curl -v https://example.com
```

### 在监听器上选择 EdgionTls

EdgionTls 通过 `parentRefs` 附加，但监听器本身也需要主动开启 EdgionTls 作为证书来源。在监听器选项中设置 `edgion.io/cert-provider` 为 `edgion-tls`，Gateway 将从匹配的 EdgionTls 解析证书（及握手选项），而不使用标准的 `tls.certificateRefs`：

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
        # 从 EdgionTls 解析证书和握手选项。
        options:
          edgion.io/cert-provider: "edgion-tls"
        # EdgionTls 提供证书时，certificateRefs 可以为空。
        certificateRefs: []
```

## 配置参数

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `parentRefs` | Array\<Object\> | none | 此配置附加的 Gateway 监听器，使用 `name` + `sectionName` 指定目标监听器 |
| `hosts` | Array\<String\> | **必填** | 此证书服务的主机名，支持单个最左侧标签通配符，如 `*.example.com` |
| `secretRef` | Object | **必填** | 持有服务端证书的 `kubernetes.io/tls` Secret，跨命名空间需要 `ReferenceGrant` |
| `clientAuth` | Object | none | mTLS 客户端证书验证；见下表 |
| `minTlsVersion` | Enum | none（BoringSSL 默认 = TLS 1.2+） | 最低接受的 TLS 版本。可选值：`TLS1_0` \| `TLS1_1` \| `TLS1_2` \| `TLS1_3` |
| `ciphers` | Array\<String\> | none（BoringSSL 默认） | TLS 1.2 及以下的 OpenSSL 格式密码套件列表，如 `ECDHE-RSA-AES256-GCM-SHA384` |

> **没有** `maxTlsVersion` 字段，密码套件是扁平的 OpenSSL 格式列表，没有"profile"选择器。请只配置上表中列出的字段。

#### `minTlsVersion` 取值说明

| 值 | TLS 版本 | 建议 |
|----|---------|------|
| `TLS1_0` | TLS 1.0 | RFC 8996 已废弃——仅限旧客户端，风险自担 |
| `TLS1_1` | TLS 1.1 | RFC 8996 已废弃——仅限旧客户端，风险自担 |
| `TLS1_2` | TLS 1.2 | 现代部署的推荐基线 |
| `TLS1_3` | TLS 1.3 | 最强安全性；所有客户端都支持时使用 |

未设置 `minTlsVersion` 时，使用 BoringSSL 默认值（TLS 1.2+），这是安全的选择。

### 安全参数——`clientAuth`（mTLS）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `clientAuth.mode` | Enum | `Terminate` | 验证模式。可选值：`Terminate` \| `Mutual` \| `OptionalMutual` |
| `clientAuth.caSecretRef` | Object | none | CA 证书 Secret（必须包含 `ca.crt` 键）。**`mode` 为 `Mutual` 或 `OptionalMutual` 时必填** |
| `clientAuth.verifyDepth` | Integer | `1` | 证书链验证深度（1–9），中间 CA 链场景需增大 |
| `clientAuth.allowedSans` | Array\<object\> | none | 带类型的 SAN 允许列表。每项包含 `type`（`DNS` \| `URI` \| `Email` \| `IP` \| `OtherName`）和 `value`。可选 `match`（默认 `Exact`，以及 `Prefix` / `Suffix` / `Contains` / `RegularExpression`）、`ignoreCase`、以及 `OtherName` 所需的 `oid`。省略或空列表表示不额外约束 SAN。DNS Exact 使用 RFC 6125 通配符。RegularExpression 在配置加载时预编译。 |
| `clientAuth.allowedCns` | Array\<String\> | none | 客户端证书 Common Name 允许列表（精确匹配） |

`allowedSans[].ignoreCase` 默认为 `false`。对于 `Exact`、`Prefix`、`Suffix`
和 `Contains` 的字符串比较，`true` 表示忽略 ASCII 大小写。DNS `Exact` 始终
使用 RFC 6125 主机名匹配；IP `Exact` 比较解析后的地址。

对于 `RegularExpression`，`ignoreCase` 使用 Unicode 简单大小写折叠设置默认
匹配模式。内联 `(?i)` 和 `(?-i)` 标志可在各自作用域内覆盖默认值：即使
`ignoreCase: false`，`(?i)^spiffe://cluster$` 也忽略大小写；而
`^spiffe://(?-i:cluster)$` 配合 `ignoreCase: true` 允许 `SPIFFE://cluster`，
但拒绝 `SPIFFE://CLUSTER`。匹配仍要求 SAN 类型一致，`OtherName` 还要求 OID
一致。正则在配置加载时编译，源码上限为 1,024 字节，程序和 DFA 上限均为
1 MiB。生效的允许列表包含非法或未编译条目时，会拒绝客户端。非法配置更新
可能按既有的最近有效配置保留规则继续使用上一份有效 TLS 配置，包括其 mTLS 策略。

#### `clientAuth.mode` 取值说明

| 值 | 含义 |
|----|------|
| `Terminate` | 单向 TLS：只验证服务端证书（默认） |
| `Mutual` | 双向 TLS：客户端**必须**提供有效证书，否则握手被拒绝 |
| `OptionalMutual` | 客户端证书可选，但提供时会被验证 |

## 常见配置场景

### 1. 单向 TLS（基础 HTTPS）

最简单的生产配置——只验证服务端证书，多个主机：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: basic-https
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - api.example.com
    - www.example.com
  secretRef:
    name: example-com-tls
    namespace: production
```

### 2. 强制双向 TLS（Mutual）

每个客户端必须提供由受信任 CA 签发的证书：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-required
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - api.example.com
  secretRef:
    name: example-com-tls
    namespace: production
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
      namespace: production
    verifyDepth: 1
```

创建 CA Secret（`ca.crt` 键是必需的）：

```bash
kubectl create secret generic client-ca \
  --from-file=ca.crt=path/to/client-ca.pem \
  -n production
```

测试：

```bash
# 带客户端证书访问（接受）
curl --cert client.crt --key client.key https://api.example.com

# 不带证书访问（在 TLS 握手时被拒绝）
curl https://api.example.com
```

### 3. 可选双向 TLS（OptionalMutual）

允许匿名客户端，但提供证书时进行验证——适用于部分已认证客户端享有额外权限的场景：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-optional
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - secure.example.com
  secretRef:
    name: example-com-tls
    namespace: production
  clientAuth:
    mode: OptionalMutual
    caSecretRef:
      name: client-ca
      namespace: production
```

### 4. mTLS + SAN / CN 允许列表

在 CA 验证之上，进一步通过 SAN 或 CN 限制允许的客户端：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-allowlist
  namespace: production
spec:
  parentRefs:
    - name: internal-gateway
      sectionName: https
  hosts:
    - admin.example.com
  secretRef:
    name: admin-example-com-tls
    namespace: production
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
      namespace: production
    allowedSans:
      - type: DNS
        value: "client1.example.com"
      - type: DNS
        value: "*.internal.example.com"
    allowedCns:
      - "AdminClient"
```

客户端只有在证书由 `client-ca` 签发**且** SAN/CN 匹配允许列表时才被接受。建议使用 `allowedSans` 而非 `allowedCns`——基于 CN 的身份标识已被废弃。

要将已验证的客户端证书身份传递到后端（供后端或插件使用），在 EdgionTls 的 metadata 中添加注解 `edgion.io/expose-client-cert: "true"`：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-allowlist
  namespace: production
  annotations:
    edgion.io/expose-client-cert: "true"
spec:
  parentRefs:
    - name: internal-gateway
      sectionName: https
  hosts:
    - admin.example.com
  secretRef:
    name: admin-example-com-tls
    namespace: production
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
      namespace: production
```

### 5. 中间 CA 证书链（verifyDepth）

当客户端证书由根 CA 下的中间 CA 签发时，需要增大验证深度：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-intermediate-ca
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - enterprise.example.com
  secretRef:
    name: enterprise-example-com-tls
    namespace: production
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: root-ca
      namespace: production
    # 允许叶证书与根 CA 之间有一层中间 CA。
    verifyDepth: 2
```

证书链结构和深度：

```text
[Client cert] <- [Intermediate CA] <- [Root CA]
```

- `verifyDepth: 1`——仅限直接签发（默认）
- `verifyDepth: 2`——一层中间 CA
- `verifyDepth: 3`——两层中间 CA

CA Secret 的 `ca.crt` 可以包含多张证书的 PEM bundle（根证书加中间证书）；bundle 中的每张证书都会加入客户端信任库。

### 6. 仅 TLS 1.3

将最低版本固定为 TLS 1.3，适用于仅限现代客户端的端点：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: tls13-only
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - secure.example.com
  secretRef:
    name: secure-example-com-tls
    namespace: production
  minTlsVersion: TLS1_3
```

仅支持 TLS 1.2 或更低版本的客户端将无法完成握手。注意 TLS 1.3 密码套件由 BoringSSL 固定，不能通过 `ciphers` 字段更改。

### 7. 自定义密码套件（TLS 1.2）

使用 OpenSSL 名称限制 TLS 1.2 密码套件：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: custom-ciphers
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - secure.example.com
  secretRef:
    name: secure-example-com-tls
    namespace: production
  minTlsVersion: TLS1_2
  ciphers:
    - ECDHE-ECDSA-AES128-GCM-SHA256
    - ECDHE-RSA-AES128-GCM-SHA256
    - ECDHE-ECDSA-AES256-GCM-SHA384
    - ECDHE-RSA-AES256-GCM-SHA384
    - ECDHE-ECDSA-CHACHA20-POLY1305
    - ECDHE-RSA-CHACHA20-POLY1305
```

`ciphers` 字段仅影响 TLS 1.2 及以下。TLS 1.3 的套件是固定的：`TLS_AES_128_GCM_SHA256`、`TLS_AES_256_GCM_SHA384`、`TLS_CHACHA20_POLY1305_SHA256`。

### 8. 多 Gateway 绑定

从多个 Gateway 提供同一证书：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: shared-tls
  namespace: production
spec:
  parentRefs:
    - name: public-gateway
      sectionName: https
    - name: internal-gateway
      sectionName: https
  hosts:
    - shared.example.com
  secretRef:
    name: shared-example-com-tls
    namespace: production
```

## 证书管理

### 滚动证书轮换（推荐）

```bash
# 1. 创建新证书 Secret
kubectl create secret tls example-com-tls-new \
  --cert=new-cert.pem --key=new-key.pem -n production

# 2. 将 EdgionTls 指向新 Secret
kubectl patch edgiontls example-tls -n production \
  --type=merge -p '{"spec":{"secretRef":{"name":"example-com-tls-new"}}}'

# 3. 确认新证书已生效
curl -v https://example.com 2>&1 | grep "expire date"

# 4. 删除旧 Secret
kubectl delete secret example-com-tls -n production
```

### 原地更新

```bash
kubectl create secret tls example-com-tls \
  --cert=new-cert.pem --key=new-key.pem -n production \
  --dry-run=client -o yaml | kubectl apply -f -
```

如需全自动签发和续期，请使用 [EdgionAcme](./acme.md)。

## 常见问题

### Q1：如何查看 Secret 中存储的证书？

**A**：使用 `kubectl` 和 `openssl` 提取并解码：

```bash
kubectl get secret example-com-tls -n production \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > cert.pem
openssl x509 -in cert.pem -noout -text
```

### Q2：通配符覆盖多少子域名层级？

**A**：精确地只有一个标签。`*.example.com` 匹配 `api.example.com`，但不匹配顶级域名 `example.com`，也不匹配 `deep.sub.example.com`。`hosts` 中只允许单个最左侧标签的通配符——如 `*.*.example.com`、`foo*.example.com` 或 `a.*.example.com` 等格式会在配置加载时因 `InvalidSanPattern` 错误而被拒绝（RFC 6125 §6.4.3）。

### Q3：`Mutual` 和 `OptionalMutual` 有什么区别？

**A**：`Mutual` 在客户端未提供（或提供了无效）证书时拒绝握手。`OptionalMutual` 允许匿名客户端通过，但提供证书时仍会验证。对于部分客户端已认证、部分未认证的混合流量使用 `OptionalMutual`。

### Q4：mTLS 可以使用自签名 CA 吗？

**A**：可以——这在开发和内部服务中很常见。将 CA 的 `ca.crt` 放入 `caSecretRef` 引用的 Secret 中。对于公开的服务端证书，请使用受信任的 CA（例如通过 [EdgionAcme](./acme.md) 和 Let's Encrypt）。

### Q5：如何仅允许 TLS 1.3？

**A**：设置 `minTlsVersion: TLS1_3`。没有 `maxTlsVersion` 字段——将最低版本固定为 TLS 1.3 实际上使监听器仅接受 TLS 1.3，因为不存在更高版本。仅支持 TLS 1.2 或以下的客户端将收到握手失败错误。

### Q6：我的 mTLS 握手失败了——从哪里开始排查？

**A**：按顺序验证：（1）客户端实际上发送了 `--cert`/`--key`；（2）客户端证书由 `caSecretRef` 中的 CA 签发，且 `ca.crt` 键存在；（3）`verifyDepth` 对证书链足够大；（4）如果设置了 `allowedSans`/`allowedCns`，客户端证书与允许列表匹配。

## 故障排查

| 症状 | 可能原因 | 解决方法 |
|------|---------|---------|
| `Secret not found` | `secretRef` 指向不存在的 Secret | 创建 Secret 或修正引用 |
| 已知良好证书的 TLS 握手失败 | SNI 主机不在 `hosts` 中，或通配符深度太深 | 将主机添加到 `hosts`；记住通配符只覆盖一个标签 |
| `Client certificate required` | `mode: Mutual` 且客户端未发送证书 | 让客户端发送 `--cert`/`--key`，或使用 `OptionalMutual` |
| CA 验证失败 | `caSecretRef` Secret 缺少 `ca.crt` 键 | 使用 `--from-file=ca.crt=...` 重新创建 Secret |
| 证书链太长 | `verifyDepth` 对中间 CA 链不够大 | 增大 `verifyDepth`（2 或 3） |
| `InvalidSanPattern` | `hosts` 通配符非法，或 `allowedSans` 的 type/match/正则/OID 校验失败 | DNS Exact 必须是 Gateway API hostname（允许 `*.example.com`）。RegularExpression 在 apply 时编译；OtherName 必须带 `oid`。 |
| 证书不生效 | EdgionTls 未附加到监听器 | 检查 `parentRefs`（`name` + `sectionName`） |

## 获取帮助

如遇问题：

1. 检查上方的常见问题和故障排查章节
2. 检查网关日志
3. 在 GitHub 提交 issue

## 相关文档

- [TLS 终结](./tls-termination.md)
- [ACME 自动证书](./acme.md)
- [HTTPS 监听器](../listeners/https.md)
- [TCP / TLS 监听器](../listeners/tcp.md)
