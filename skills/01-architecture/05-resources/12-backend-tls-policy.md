---
name: resource-backend-tls-policy
description: BackendTLSPolicy 资源：上游 mTLS 配置、CA 证书解析、客户端证书。
---

# BackendTLSPolicy 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

BackendTLSPolicy 是 Gateway API 标准的策略资源，用于配置 Gateway 到后端 Service 的上游 TLS 连接，包括 CA 证书验证和客户端证书（mTLS）。它通过 `targetRef` 绑定到特定 Service。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs`
- Gateway ConfHandler: `src/core/gateway/backends/policy/backend_tls/conf_handler_impl.rs`
- 类型定义: `src/types/resources/backend_tls_policy.rs`

## Controller 侧处理

### parse

1. **清除旧引用**：清除 SecretRefManager 中该 BackendTLSPolicy 的旧记录
2. **解析 CA 证书 Secret**：遍历 `spec.validation.caCertificateRefs`，对每个 kind=Secret 的引用：
   - 在 SecretRefManager 注册引用关系
   - 从 GLOBAL_SECRET_STORE 读取 Secret 数据，填入 `spec.resolved_ca_certificates`
3. **解析客户端证书 Secret**：通过 `client_certificate_secret_ref()` 获取客户端证书引用：
   - 在 SecretRefManager 注册引用关系
   - 从 GLOBAL_SECRET_STORE 读取 Secret 数据，填入 `spec.resolved_client_certificate`

### on_delete

清除 SecretRefManager 引用。

### update_status

使用合成的 PolicyAncestorStatus（因 BackendTLSPolicy 的 target 是 Service 而非 Gateway）：
- `Accepted`：无 validation_errors 时为 True
- `ResolvedRefs`：校验所有 caCertificateRef 的 kind/group 合法性、CA Secret 是否已解析、客户端证书 Secret 是否存在且包含 tls.crt 和 tls.key
- 不设置 Programmed/Ready Condition（需要数据面反馈，当前架构不支持）

## Gateway 侧处理

BackendTLSPolicy 同步到 Gateway 后，在向后端 Service 发起连接时：
- 使用 `resolved_ca_certificates` 中的 CA 证书验证后端的服务端证书
- 使用 `resolved_client_certificate` 中的客户端证书向后端提供身份证明（mTLS）

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| BackendTLSPolicy → Secret | Secret | caCertificateRefs | CA 证书用于验证后端服务端证书 |
| BackendTLSPolicy → Secret | Secret | clientCertificateRef | 客户端证书用于 mTLS |
| BackendTLSPolicy → Service | Service | targetRef | 策略绑定的目标 Service |
| BackendTLSPolicy ← SecretRefManager | Secret | 级联 requeue | Secret 变更时自动重新处理 |
