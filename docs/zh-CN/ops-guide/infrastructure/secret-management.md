# Secret 管理

Edgion 将 Kubernetes Secret 用于 TLS 证书、插件凭据和 Controller 管理员密码。本页介绍 Secret 的引用方式、涉及的 Secret 类型、自动生成的管理员凭据以及运维建议。

## Edgion 使用 Secret 的场景

| 用途 | 引用位置 | 典型 Secret 类型 |
|------|---------|-----------------|
| Gateway TLS 证书 | Gateway 的 `listeners[].tls.certificateRefs` | `kubernetes.io/tls` |
| 扩展 TLS（mTLS、密码套件、OCSP） | `EdgionTls.spec.secretRef` 和 `clientAuth.caSecretRef` | `kubernetes.io/tls`（服务端）、`Opaque`（CA） |
| 插件凭据 | `jwt-auth`、`basic-auth`、`openid-connect` 等插件 | `Opaque` |
| 后端认证 | 后端 mTLS 客户端证书 | `kubernetes.io/tls` |
| Controller 管理员密码 | 自动生成的 `edgion-admin-credentials` | `Opaque` |

### TLS 证书 Secret

标准 Kubernetes TLS Secret 包含 `tls.crt` 和 `tls.key`：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: my-tls-secret
  namespace: production
type: kubernetes.io/tls
data:
  tls.crt: <base64-encoded-cert>
  tls.key: <base64-encoded-key>
```

用于 mTLS 客户端验证的 CA Secret 持有 CA 证书，通常是包含 `ca.crt` 键的 `Opaque` Secret：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
  namespace: production
type: Opaque
data:
  ca.crt: <base64-encoded-ca-cert>
```

⚠️ 切勿将真实证书或密钥材料提交到源代码控制。上方所有值均为占位符。

## 管理员凭据（自动生成）

当 Controller 在**未配置** `[local_auth]` 段的情况下启动时，会自动生成随机管理员密码，并存储在 `edgion-system` 命名空间中名为 `edgion-admin-credentials` 的 Secret 里。Kubernetes 模式和本地模式均适用。

获取生成的密码：

```bash
# Kubernetes 模式
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.password}' | base64 -d

# 本地模式（基于文件的配置）
cat {conf_dir}/Secret_edgion-system_edgion-admin-credentials.yaml
```

> **Center 与此不同**：Center 进程**不会**自动生成凭据。运维人员必须提供 `[local_auth]` 或 `[auth]`；否则业务路由以 503 失败关闭，而健康检查端点保持可用。

## 管理建议

1. 按用途拆分 Secret，不要把无关凭据塞进一个对象。
2. 使用最小权限 RBAC，限制各 Secret 的可见范围。
3. 在生产环境启用 Secret 轮换策略。
4. 变更 Secret 后，观察引用资源的状态和网关日志，确认变更已生效。

## 故障排查

### 引用 Secret 后 Gateway 或 Route 状态异常

- 确认 Secret 名称和命名空间正确。
- 当 Secret 在其他命名空间时，确认 [ReferenceGrant](./reference-grant.md) 已授权该引用。
- 确认 Secret 键名与消费方期望一致（TLS 使用 `tls.crt`/`tls.key`，CA 使用 `ca.crt`，插件使用各自特定的键）。

### TLS 握手失败

- 确认证书链与私钥匹配。
- 确认证书未过期。
- 确认客户端发送的 SNI 与证书 SAN 匹配。

### 无法登录管理 API

- 从 `edgion-admin-credentials` 获取自动生成的密码（见上文），或显式配置 `[local_auth]`。

## 常见问题

### Q1：Controller 管理员密码存储在哪里？

**A**：存储在 `edgion-system` 命名空间的 `edgion-admin-credentials` Secret 中，在首次未配置 `[local_auth]` 的情况下启动时自动生成。本地模式下写入 `{conf_dir}/Secret_edgion-system_edgion-admin-credentials.yaml`。

### Q2：可以引用其他命名空间的 Secret 吗？

**A**：可以，但必须在 Secret 所在命名空间创建 `ReferenceGrant` 以授权引用。详见[跨命名空间引用](./reference-grant.md)。

### Q3：我的插件报告缺少凭据，但 Secret 已存在。为什么？

**A**：Secret 内部的键名必须与插件期望的字段名匹配。查阅插件文档了解所需键，并确认它们在 Secret 的 `data` 中存在（且为 base64 编码）。

## 相关文档

- [TLS 终结](../gateway/tls/tls-termination.md)
- [EdgionTls](../gateway/tls/edgion-tls.md)
- [mTLS 配置](./mtls.md)
- [跨命名空间引用](./reference-grant.md)
