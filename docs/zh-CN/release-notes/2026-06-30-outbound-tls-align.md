# 出站 TLS — BackendTLSPolicy Schema 对齐（破坏性变更）

**发布日期**: 2026-06-30  
**类型**: 破坏性变更  
**影响范围**: 插件 (OpenID Connect、LDAP Auth、Ext Proc)、LinkSys 提供商 (Redis、etcd、Elasticsearch)、EdgionGatewayConfig 全局默认配置

## 概述

用于插件、链接系统提供商和全局网关配置的出站 TLS schema 已重塑，以对齐 Gateway API 的 `BackendTLSPolicy` 模型。这是一项破坏性变更：操作员必须将现有配置从旧的 `caSource` / `caSecretRef` / `clientSecretRef` 模型迁移到新的 `validation.caCertificateRefs` / `clientCertificateRef` 形式。

## 字段映射

| 已删除 | 新字段 | 说明 |
|---|---|---|
| `tls.caSource: SecretRef` + `tls.caSecretRef: {name: X}` | `tls.validation.caCertificateRefs: [{kind: Secret, name: X}]` | CA 信任源现在使用 `caCertificateRefs`（支持多个引用） |
| `tls.caSource: System` | `tls.validation.wellKnownCACertificates: System` 或完全省略 `validation` | 省略 `validation` 时，默认使用系统 webpki 根证书 |
| `tls.caSource: Inline` + `tls.caCert: "<PEM>"` | 不支持；改用 Secret 或 ConfigMap 存储 CA 证书 | 配置中的内联 CA 证书已不再支持 |
| `tls.clientSecretRef: {name: Y}` | `tls.clientCertificateRef: {name: Y}` | 字段已重命名；语义不变 |
| `tls.insecureSkipVerify: true` | `tls.verify: false` | 三态外部开关；替代了旧的三态 `caSource` 模型 |

## 新能力

### 多个 CA Bundle

`caCertificateRefs` 字段现在接受多个证书引用列表。如果服务器证书可链接到任何提供的 CA，验证即成功：

```yaml
tls:
  verify: true
  validation:
    caCertificateRefs:
      - kind: Secret
        name: internal-ca
      - kind: Secret
        name: partner-ca
```

### ConfigMap CA 源

除了 Secret，ConfigMap 现在也可以作为 CA 证书源（插件 oidc、ldap、ext_proc 和 EdgionGatewayConfig 的新能力）。LinkSys 提供商（Redis、etcd、Elasticsearch）目前仅支持 Secret 引用。

```yaml
tls:
  verify: true
  validation:
    caCertificateRefs:
      - kind: ConfigMap
        name: corporate-ca-bundle
```

### SAN 主机名固定（Schema 仅，Phase 1）

Schema 现在暴露了可选的主机名和 Subject Alternative Name (SAN) 字段：

```yaml
tls:
  verify: true
  validation:
    caCertificateRefs:
      - kind: Secret
        name: idp-ca
    hostname: idp.example.com
    subjectAltNames:
      - type: Hostname
        hostname: idp.example.com
```

这些字段在本版本中**仅在 schema 级别验证**。执行层面的 SAN 固定验证将在后续版本为 OpenID Connect 和 LDAP Auth 提供商实现。如果在尚不支持 SAN 固定的提供商上配置这些字段，资源将显示 `Accepted=False`，并提供说明该限制的原因。

## 迁移指南

### 步骤 1：识别受影响的资源

找到所有使用旧出站 TLS schema 的资源：

```bash
# 检查 EdgionGatewayConfig
kubectl get egc -A -o yaml | grep -A 10 "caSource:"

# 检查插件 (oidc, ldap, ext_proc)
kubectl get edgionplugins -A -o yaml | grep -A 10 "caSource:"

# 检查 LinkSys 提供商
kubectl get edgionlinksys -A -o yaml | grep -A 10 "caSource:"
```

### 步骤 2：迁移配置

使用上面的字段映射表重写配置。示例：

**迁移前（旧 schema）：**

```yaml
apiVersion: edgion.io/v1beta1
kind: EdgionPlugin
metadata:
  name: ldap-auth
spec:
  type: ldap_auth
  config:
    ca_source: SecretRef
    ca_secret_ref:
      name: ldap-ca
    client_secret_ref:
      name: ldap-client-cert
    insecure_skip_verify: false
```

**迁移后（新 schema）：**

```yaml
apiVersion: edgion.io/v1beta1
kind: EdgionPlugin
metadata:
  name: ldap-auth
spec:
  type: ldap_auth
  config:
    validation:
      caCertificateRefs:
        - kind: Secret
          name: ldap-ca
    clientCertificateRef:
      name: ldap-client-cert
    verify: true
```

### 步骤 3：内联 CA 迁移

如果之前使用内联 CA 证书（`caSource: Inline`），请迁移到 Kubernetes Secret：

1. 从配置中提取 CA 证书
2. 创建 Kubernetes Secret：

```bash
kubectl create secret generic my-ca \
  --from-literal=ca.crt="$(cat /path/to/ca.pem)" \
  -n <target-namespace>
```

3. 更新资源以引用 Secret：

```yaml
tls:
  verify: true
  validation:
    caCertificateRefs:
      - kind: Secret
        name: my-ca
```

或使用 ConfigMap：

```bash
kubectl create configmap my-ca \
  --from-literal=ca.crt="$(cat /path/to/ca.pem)" \
  -n <target-namespace>
```

## 行为变更

### Schema 验证

使用旧字段的过时配置将按以下方式处理：

- **结构化 CRD 路径**（LinkSys 的 `tls` 块、EdgionGatewayConfig 的 `outboundTls`）：Kubernetes API 服务器将**静默删除**任何未在 CRD 结构化 schema 中声明的字段。旧字段不会导致显式错误，而是被静默忽略（变成无操作）。使用 `kubectl get -o yaml` 检查部署的配置以验证迁移是否完成。

- **插件路径**（EdgionPlugins 资源，位于 `x-kubernetes-preserve-unknown-fields` 下）：Schema 将在插件验证期间强制执行。旧字段将导致资源进入 `Accepted=False` 状态，并显示明确的错误消息，说明过时的字段。

### 验证行为

- `tls.verify: false` 继续作为操作员的逃生舱用于开发/测试场景。配置后，整个 `validation` 块被忽略，跳过证书验证。
- 当 `tls.verify: true`（默认）或省略时，`validation` 块被采用。
- 如果省略 `validation` 且 `verify` 不是 `false`，网关默认使用系统 webpki 根证书进行验证。

### LinkSys 限制

LinkSys 提供商（Redis、etcd、Elasticsearch）目前仅支持 Secret 引用。ConfigMap CA 源尚未支持 LinkSys。所有 LinkSys TLS CA 引用请使用 `kind: Secret`。

## 相关文档

有关详细的 schema 设计原理和 Phase 2 路线图（SAN 固定执行），请参见设计规范：[`docs/superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md`](../../../superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md)

## 支持与问题

如果在迁移过程中遇到问题，请检查：

1. **配置状态**：`kubectl describe` 受影响的资源，查看任何 `Accepted=False` 条件
2. **准入日志**：在结构化路径中查找被删除字段的日志
3. **网关日志**：TLS 连接失败会显示在网关审计/访问日志中，包含稳定的 `EdgionErrorCode` 诊断和原因
