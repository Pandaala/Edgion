# Secret 管理

Edgion 将 Kubernetes Secret 用于 TLS 证书、插件凭据和 Controller 管理员密码。本页介绍 Secret 的引用方式、涉及的 Secret 类型、自动生成的管理员凭据以及运维建议。

## Edgion 使用 Secret 的场景

| 用途 | 引用位置 | 典型 Secret 类型 |
|------|---------|-----------------|
| Gateway TLS 证书 | Gateway 的 `listeners[].tls.certificateRefs` | `kubernetes.io/tls` |
| 扩展 TLS（mTLS、密码套件、OCSP） | `EdgionTls.spec.secretRef` 和 `clientAuth.caSecretRef` | `kubernetes.io/tls`（服务端）、`Opaque`（CA） |
| 插件凭据 | `jwt-auth`、`basic-auth`、`openid-connect` 等插件 | `Opaque` |
| 后端认证 | 后端 mTLS 客户端证书 | `kubernetes.io/tls` |
| Controller 管理员密码 | 运维人员创建的 `edgion-admin-credentials`（注入为环境变量） | `Opaque` |

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

## 管理员凭据（`edgion-admin-credentials`）

Controller 从 `edgion-admin-credentials` Secret（命名空间：`edgion-system`）读取管理员凭据。
该 Secret 必须由运维人员创建——Controller 不再自动生成（已于 2026-06-01 移除）。

Kubernetes 推荐方式是使用 `deploy.sh`，该脚本会幂等地创建 Secret 并通过环境变量注入到 controller Deployment：

```bash
# 运行 deploy.sh 后读取管理员密码
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.password}' | base64 -d
```

也可以在挂载为 Secret 的配置文件中配置 `[local_auth]`，或使用 `[auth]` OIDC。

若未配置任何有效凭据，服务器仍可启动，但所有 Admin API 业务路由返回 **503 fail-close**。
`/health`、`/ready`、`/metrics` 和 `/api/v1/auth/status` 保持可访问。Controller 和 Center 遵循相同的行为。

完整引导矩阵参见 `skills/02-features/02-config/03-auth-bootstrap.md`。

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

- 从 `edgion-admin-credentials` 获取密码（见上文——由 `deploy.sh` 创建），或显式配置 `[local_auth]` / `[auth]`。

## 常见问题

### Q1：Controller 管理员密码存储在哪里？

**A**：存储在 `edgion-system` 命名空间的 `edgion-admin-credentials` Secret 中，由运维人员通过 `deploy.sh` 创建。controller Deployment 将 Secret 中的值以环境变量（`EDGION_ADMIN_USERNAME`、`EDGION_ADMIN_PASSWORD`、`EDGION_ADMIN_JWT_SECRET`）形式注入。Controller 不再自动生成该 Secret。

### Q2：可以引用其他命名空间的 Secret 吗？

**A**：可以，但必须在 Secret 所在命名空间创建 `ReferenceGrant` 以授权引用。详见[跨命名空间引用](./reference-grant.md)。

### Q3：我的插件报告缺少凭据，但 Secret 已存在。为什么？

**A**：Secret 内部的键名必须与插件期望的字段名匹配。查阅插件文档了解所需键，并确认它们在 Secret 的 `data` 中存在（且为 base64 编码）。

## 相关文档

- [TLS 终结](../gateway/tls/tls-termination.md)
- [EdgionTls](../gateway/tls/edgion-tls.md)
- [mTLS 配置](./mtls.md)
- [跨命名空间引用](./reference-grant.md)
