---
name: resource-backend-tls-policy
description: BackendTLSPolicy 资源：上游 mTLS 配置、CA 证书解析、客户端证书、冲突检测。
---

# BackendTLSPolicy 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

BackendTLSPolicy 是 Gateway API 标准的策略资源，用于配置 Gateway 到后端 Service 的上游 TLS 连接，包括 CA 证书验证和客户端证书（mTLS）。它通过 `targetRef` 绑定到特定 Service。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs`
- 冲突检测: `src/core/controller/conf_mgr/sync_runtime/resource_processor/backend_tls_policy_ref.rs`
- Gateway ConfHandler: `src/core/gateway/backends/policy/backend_tls/conf_handler_impl.rs`
- 类型定义: `src/types/resources/backend_tls_policy.rs`

## Controller 侧处理

### parse

1. **冲突检测注���**：
   - 计算 old siblings（清除前，用于 requeue）
   - 清除 `BackendTLSPolicyTargetManager` 中旧的 target 注册
   - 遍历 `spec.target_refs`，按 `{ns}/{svc_name}[::sectionName]` 格式生成 conflict key 并注册
   - 存储 `creationTimestamp` 用于冲突优先级排序
   - requeue old siblings（target 变更时旧 siblings 需重新评估冲突状态）
2. **清除旧引用**：清除 SecretRefManager 中该 BackendTLSPolicy 的旧记录
3. **解析 CA 证书**：遍历 `spec.validation.caCertificateRefs`，对每个 kind=Secret 或 kind=ConfigMap 的引用：
   - 在 SecretRefManager 注册引用关系
   - 从 GLOBAL_SECRET_STORE / ConfigMap Store 读��数据，填入 `spec.resolved_ca_certificates`
4. **解析客户端证书 Secret**：通过 `client_certificate_secret_ref()` 获取客户端证书引用：
   - 在 SecretRefManager 注册引用关系
   - 从 GLOBAL_SECRET_STORE 读取 Secret 数据，填入 `spec.resolved_client_certificate`

### on_change

requeue 当前 siblings（共享同一 target 的其他 BackendTLSPolicy），确保它们的冲突状态被重新评估。

### on_delete

1. 找到 siblings（清除前）
2. 清除 `BackendTLSPolicyTargetManager` 中的 target 注册和时间戳
3. 清除 SecretRefManager 引用
4. requeue siblings（删除后之前 Conflicted 的 sibling 可能变为 Accepted）

### update_status

使用合成的 PolicyAncestorStatus（因 BackendTLSPolicy 的 target 是 Service 而非 Gateway）：
- `Accepted`（优先级：Invalid > NoValidCACertificate > Conflicted > Accepted）：
  - validation_errors 不为空 → `False, Reason=Invalid`
  - 所有 CA refs 都无效（kind 错误 + ref 未解析 ≥ 总 ref 数） → `False, Reason=NoValidCACertificate`
  - 冲突检测：遍历 target refs 调用 `is_policy_winner()`，非 winner → `False, Reason=Conflicted`
  - 其他 → `True, Reason=Accepted`
- `ResolvedRefs`：校验所有 caCertificateRef 的 kind/group 合法性、CA Secret/ConfigMap 是否已解析、客户端证书 Secret 是否存在且包含 tls.crt 和 tls.key
- 不设置 Programmed/Ready Condition（需要数据面反馈，当前架构不支持）

**已修复**：`ancestorRef` 通过 ServiceRefManager + GatewayRouteIndex 做 `Service → Route → Gateway` 三步查询，正确指向关联的 Gateway。Route on_change/on_delete 自动 requeue 关联的 BackendTLSPolicy 以保持 ancestor 状态一致。

### 冲突检测机制

使用 `BackendTLSPolicyTargetManager`（`BidirectionalRefManager<ResourceRef>` 实例），与 `ServiceRefManager`、`SecretRefManager` 同一 pattern：

- **Conflict key**: `"{ns}/{svc_name}"` 或 `"{ns}/{svc_name}::{sectionName}"`，不同 sectionName 不冲突
- **优先级**: creationTimestamp ASC → `{namespace}/{name}` 字典序，第一个为 winner
- **Sibling requeue**: parse 阶段 requeue old siblings，on_change 阶段 requeue new siblings，on_delete 阶段 requeue 所有 siblings

## Gateway 侧处理

BackendTLSPolicy 同步到 Gateway 后，在向后端 Service 发起连接时：
- 使用 `resolved_ca_certificates` 中的 CA 证书验证后端的服务端证书
- 使用 `resolved_client_certificate` 中的客户端证书向后端提供身份证明（mTLS）

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| BackendTLSPolicy → Secret | Secret | caCertificateRefs | CA 证书用于验证后端服务端证书 |
| BackendTLSPolicy → ConfigMap | ConfigMap | caCertificateRefs | CA 证书（ConfigMap 形式） |
| BackendTLSPolicy → Secret | Secret | clientCertificateRef | 客户端证书用于 mTLS |
| BackendTLSPolicy → Service | Service | targetRef | 策略绑定的目标 Service |
| BackendTLSPolicy ↔ BackendTLSPolicy | BackendTLSPolicy | BackendTLSPolicyTargetManager | 同 target 冲突检测 + sibling requeue |
| BackendTLSPolicy ← SecretRefManager | Secret | 级联 requeue | Secret 变更时自动重新处理 |
