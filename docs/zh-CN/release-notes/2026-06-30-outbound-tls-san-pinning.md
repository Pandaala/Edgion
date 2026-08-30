# 出站 TLS SAN 固定执行 — OIDC + LDAP (Phase 2)

**发布日期**: 2026-06-30  
**类型**: 功能启用（对于预配置 SAN 的 OIDC/LDAP 具有破坏性）  
**影响范围**: OIDC Connect 和 LDAP Auth 插件；LinkSys 和 EdgionGatewayConfig 全局默认仍处于延期阶段

## 概述

Phase 2 在 Phase 1 schema 对齐的基础上，通过**启用 OIDC 和 LDAP 插件的运行时 SAN 固定验证**来进一步推进。之前，在这些插件上配置 `validation.hostname` 或 `validation.subjectAltNames` 会导致 `Accepted=False`。现在，控制器接受该配置，网关在 TLS 握手时使用 rustls 的 `OutboundSanVerifier` 强制 SAN 匹配。证书不匹配会导致握手失败，并记录 `OutboundTlsSanMismatch` 错误诊断。

其他后端——LinkSys 提供商（Redis、etcd、Elasticsearch、Kafka）、ext_proc 和 EdgionGatewayConfig 全局默认——仍延期到后续版本。在这些不支持的后端上配置 SAN 会继续导致 `Accepted=False`。

## 行为变更

### OIDC 和 LDAP 插件

**之前（Phase 1）：**
- 在 OIDC 或 LDAP auth 插件上配置 `validation.subjectAltNames` 或 `validation.hostname` 会导致 `Accepted=False`，原因为"尚不支持 SAN 固定"。
- 不进行任何 TLS 验证；该配置实际被忽略。

**现在（Phase 2）：**
- `validation.hostname` 和 `validation.subjectAltNames` 被**接受并在运行时强制执行**。
- 在 TLS 握手到 OIDC 提供商（如 Keycloak、Okta、Azure AD）或 LDAP 服务器时，网关的 rustls 验证器检查对端证书的 Subject Alternative Names (SANs) 是否与配置的值匹配。
- 如果验证失败（证书 SAN 不包含预期的主机名或值），握手立即中止。连接会在网关访问日志中记录 `EdgionErrorCode::OutboundTlsSanMismatch`。
- 握手失败会传播到请求：客户端收到错误响应，网关记录该拒绝（详见下方"诊断"章节）。

### LinkSys、ext_proc 和全局默认（仍处于延期阶段）

- **Redis、etcd、Elasticsearch、Kafka**：SAN 固定配置会继续导致 `Accepted=False`。有关执行时间表，请参见 `tasks/todo/outbound-tls-sdk-san-audit.md`。
- **ext_proc**：SAN 固定配置导致 `Accepted=False`；执行被延期。
- **EdgionGatewayConfig 全局 `outboundTls` 默认**：全局级别通过 `validation.hostname` 或 `validation.subjectAltNames` 的 SAN 固定尚未执行；资源回退到提供商特定的覆盖。

## 新错误代码

| Kind | Code | 常见 HTTP 结果 | 条件 |
|------|------|---------------|------|
| `error` | `OutboundTlsSanMismatch` | 502 Bad Gateway | 出站对端证书的 Subject Alternative Names 与配置的 `validation.subjectAltNames` 或 `validation.hostname` 不匹配 |

> **范围说明。** `OutboundTlsSanMismatch` 会出现在 LDAP bind 的访问日志中；OIDC 一侧只有交互式登录
> 跳转和 callback 处理这两个入口触发的 discovery 请求会写入该状态。其余所有 OIDC 出站调用
> （token-exchange、refresh、revoke、introspection、userinfo，以及从这些路径触发的 discovery 请求）
> 只记录 `edgion_gateway_outbound_tls_san_mismatch_total` 计数器，不写入单次请求的状态。
> SAN 不匹配属于配置错误，会使每个请求以相同方式失败，因此该计数器是所有路径上的预期告警信号。

## 操作员操作必需

### ⚠️ 影响评估

如果您在 OIDC 或 LDAP auth 插件上**预先配置了 `validation.subjectAltNames` 或 `validation.hostname`**，这些配置现在在运行时**变为活跃**。TLS 握手将强制 SAN 匹配。如果配置的值与目标服务的证书 SANs 不匹配，握手将失败。

### 步骤 1：审计现有配置

找到所有带有 SAN 配置的 OIDC 和 LDAP 插件：

```bash
kubectl get edgionplugins -A -o yaml | grep -B 5 "subjectAltNames\|hostname" | grep -B 5 "ldap_auth\|openid_connect"
```

### 步骤 2：验证证书 SANs

对于每个 OIDC 提供商和 LDAP 服务器，获取服务器证书并检查其 SANs：

**对于 OIDC（HTTPS 端点）：**

```bash
echo | openssl s_client -servername idp.example.com -connect idp.example.com:443 2>/dev/null | \
  openssl x509 -noout -text | grep -A 1 "Subject Alternative Name"
```

**对于 LDAP（启用 TLS 的 LDAP 服务器）：**

```bash
echo | openssl s_client -connect ldap.example.com:636 2>/dev/null | \
  openssl x509 -noout -text | grep -A 1 "Subject Alternative Name"
```

确保证书中的 SANs 与您在 `validation.subjectAltNames` 或 `validation.hostname` 中配置的值匹配。

### 步骤 3：监控握手失败

部署 Phase 2 后，对 `edgion_gateway_outbound_tls_san_mismatch_total` 计数器设置告警——这是预期的监控信号，
诊断码是稳定字符串，没有数字表示：

```bash
# 对出站 TLS SAN 固定拒绝设置告警
sum(rate(edgion_gateway_outbound_tls_san_mismatch_total[5m])) by (plugin) > 0
```

用于临时日志排查：

```bash
kubectl logs -n edgion-system -l app=edgion-gateway -c edgion-gateway | grep "OutboundTlsSanMismatch"
```

如果看到失败，调整配置以匹配证书的实际 SANs，或临时删除 SAN 固定以解除生产阻塞（并提交问题）。

### 步骤 4：可选——主动使用 SAN 固定

如果您尚未在 OIDC 或 LDAP 上配置 SAN 固定，这是一个很好的机会来添加它作为安全最佳实践：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugin
metadata:
  name: corporate-oidc
  namespace: edgion-system
spec:
  type: openid_connect
  config:
    issuer: https://idp.example.com
    clientId: my-client-id
    clientSecretRef:
      name: oidc-client-secret
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

## 设计参考

有关详细的原理和完整的三阶段执行时间表（OIDC/LDAP 在 Phase 2，LinkSys/ext_proc/全局默认在 Phase 3），请参见：

- [`docs/superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md`](../../../superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md) § 6
- [`tasks/todo/outbound-tls-sdk-san-audit.md`](../../../tasks/todo/outbound-tls-sdk-san-audit.md)（执行审计和时间表）

## 诊断

### 访问日志条目

当握手因 SAN 不匹配而失败时，访问日志的 `diagnostics` 数组会包含对应的错误诊断：

```json
{
  "diagnostics": [{ "kind": "error", "code": "OutboundTlsSanMismatch" }]
}
```

### SSL 日志（网关数据平面）

`ssl.log` 只记录*入站* TLS 握手诊断信息（client → gateway），不涉及出站 SAN 固定。对于出站 TLS SAN
不匹配，请使用上文描述的计数器以及访问日志诊断项。

### 调试命令

```bash
# 追踪访问日志中的最近失败
kubectl logs -n edgion-system -l app=edgion-gateway --tail=100 -f | grep "OutboundTlsSanMismatch"

# 描述插件以查看其配置
kubectl describe edgionplugin corporate-oidc -n edgion-system

# 检查插件的 Accepted 条件
kubectl get edgionplugin corporate-oidc -n edgion-system -o jsonpath='{.status.conditions[?(@.type=="Accepted")]}'
```

## 迁移检查清单

- [ ] 运行审计命令找到带有 SAN 配置的 OIDC/LDAP 插件
- [ ] 从所有配置的 OIDC 提供商和 LDAP 服务器获取证书
- [ ] 验证证书中的 SANs 与配置的 `validation.subjectAltNames` 和 `validation.hostname` 匹配
- [ ] 在 canary 环境中部署 Phase 2 并对 `edgion_gateway_outbound_tls_san_mismatch_total` 设置告警
- [ ] 根据需要调整配置或证书以解决 SAN 不匹配问题
- [ ] 自信地部署到生产环境

## 相关文档

有关 OIDC 和 LDAP 插件配置的详细信息，请参见 Edgion 用户指南中的插件文档。关于出站 TLS SAN 固定故障排除，请查阅网关访问日志和 `edgion_gateway_outbound_tls_san_mismatch_total` 计数器，如上面诊断部分所述（ssl.log 只覆盖入站 TLS 握手）。
