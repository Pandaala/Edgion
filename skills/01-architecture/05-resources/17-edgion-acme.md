---
name: resource-edgion-acme
description: EdgionAcme 资源：ACME 自动证书签发、DNS/HTTP-01 挑战、Leader-only 执行。
---

# EdgionAcme 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

EdgionAcme 是 Edgion 的自定义扩展资源，实现 ACME 协议（Automatic Certificate Management Environment）的自动证书签发。支持 Let's Encrypt 等 ACME CA，通过 HTTP-01 或 DNS-01 挑战验证域名所有权，自动签发和续期 TLS 证书。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_acme.rs`
- ACME Service: `src/core/controller/services/acme/`
- Gateway ConfHandler: `src/core/gateway/services/acme/conf_handler_impl.rs`
- 类型定义: `src/types/resources/edgion_acme.rs`

## Controller 侧处理

### Spec / Status 职责边界

`EdgionAcmeSpec` 仅承载 **operator 输入**：`server` / `email` / `privateKeySecretRef` / `domains` / `keyType` / `challenge` / `renewal` / `externalAccountBinding` / `storage` / `autoEdgionTls`。**没有任何 controller-managed 字段**。

`EdgionAcmeStatus` 是 controller 观测通道：除生命周期字段（`phase`、`certificateSerial`、`certificateNotAfter`、`lastRenewalTime`、`lastFailureReason` 等）外，还有 `activeChallenges`——HTTP-01 挑战 token 列表，由 `AcmeService` 通过独立 fieldManager `edgion-acme-challenges` 经 `/status` 子资源 SSA 写入；Gateway 通过 `EdgionAcme.active_challenges()` helper 读取（指向 `status.activeChallenges`）来 serve `/.well-known/acme-challenge/<token>`。

`AcmeChallenge` 是 `#[serde(tag = "type")]` 内部标签枚举：`type: http-01` 携带 `gatewayRef`；`type: dns-01` 携带 `provider` / `credentialRef` / `propagationTimeout` / `propagationCheckInterval`。两组字段类型级互斥——不存在 "type=http-01 但写了 dns01 字段" 的非法状态。

### validate

1. **email**：必填，用于 ACME 账户注册
2. **domains**：至少一个域名
3. **通配符域名校验**：`*.example.com` 格式的通配符域名必须使用 DNS-01 挑战类型
4. **DNS-01 provider 校验**：仅当 challenge 是 DNS-01 时检查 provider 名称（支持 cloudflare、alidns）。HTTP-01 / DNS-01 字段完整性由 `AcmeChallenge` 标签枚举在反序列化期保证，不需要运行时检查

### parse

Resolved Secret 字节 **不**写回内存中的 `EdgionAcme`——`AcmeService` 在签发/续期时通过 K8s API 重新拉取（`get_or_create_acme_account` / `fetch_dns_credentials`）。Handler 侧的 resolve 仅用于 `ResolvedRefs` condition + `SecretRefManager` 注册（让 Secret 变更触发 requeue）。

1. **清除旧引用**：清除 SecretRefManager 中该 EdgionAcme 的旧记录
2. **解析 ACME 账户 Secret**（`spec.privateKeySecretRef`，必填）：
   - 走 credential pipeline（`resource_processor::credential::pipeline::resolve_secret` + `AcmeAccountResolver`）
   - Secret 允许 `Opaque` 类型，必需 key `accountCredentials`（JSON，即 `instant_acme::AccountCredentials`）
   - 若 Secret 尚不存在（初次装机 / 等待 `AcmeService` 回写），会落 `ResolvedRefs=False/SecretNotFound`，`AcmeService` 生成账户后会写入 Secret 并触发 requeue
3. **解析 DNS 凭证 Secret**（仅 DNS-01 模式）：
   - 从 `dns01.credentialRef` 获取 Secret 引用（`SecretObjectReference` 结构，沿用 Gateway API 风格，不经过统一 `SecretMaterialResolver`，因为键名随 provider 不同：cloudflare = `api-token`，alidns = `access-key-id` + `access-key-secret`）
   - 在 SecretRefManager 注册引用关系；validate 失败 / DecodeError 等情况映射成 `ResolvedRefs` condition
4. **通知 ACME Service**：调用 `notify_resource_changed()` 通知后台 ACME Service 有资源变更，触发证书签发/续期流程

### on_delete

清除 SecretRefManager 引用。

### update_status

处理器侧（`EdgionAcmeHandler::update_status`）**只**管理 Gateway API 风格的 `Accepted` condition：

- validation_errors 空 → `Accepted=True/Accepted`
- validation_errors 非空 → `Accepted=False/Invalid`，message 是错误拼接
- 同时保留 `last_failure_reason` 文本字段供仍读它的老工具使用

**phase 及其他 ACME 生命周期字段**（`certificate_serial`、`last_renewal_time`、`secret_name` 等）不由 handler 写，而是由后台 `AcmeService` 通过 Server-Side Apply（field manager `edgion-acme-controller`）独立更新。三组写入者字段所有权不重叠：
- Handler 写 `status.conditions`（Merge Patch）
- AcmeService 写 `status.phase` 及相关字段（SSA，fieldManager `edgion-acme-controller`；payload 中 `conditions` 因 `skip_serializing_if = "Vec::is_empty"` 被忽略，不会清空 handler；`activeChallenges` 同样因为 `Vec::is_empty` skip 不会被这条 SSA 触碰）
- AcmeService 写 `status.activeChallenges`（SSA，fieldManager `edgion-acme-challenges`，独立路径，仅 HTTP-01 在飞期间有值）

SSA + handler Merge Patch 混写存在理论上的"processor 处理事件时 watch 尚未反映 AcmeService 最新 phase"的短暂回滚窗口。这同样适用于 `activeChallenges`：handler 的 Merge Patch payload 是把 in-memory `EdgionAcmeStatus` 整体 serialize 后写到 `/status`，包含从 watch event 反序列化得到的 `activeChallenges`（非空时）。如果 handler 在处理 RV=N 的事件时 AcmeService 在 RV=N+1 写了新一轮 challenges，handler 的 Merge Patch 会把 `activeChallenges` 回退到 RV=N 的版本。实际上：

- AcmeService 只在 HTTP-01 phase 2 push 一次、phase 6 clear 一次，期间无中间写入，所以正常 issuance 不触发竞态；
- 唯一会触发的是「flow 进行中 operator 同时改 spec」这种罕见组合；
- 自愈路径：下一次 watch event handler 重读最新 `activeChallenges` 后写回，AcmeService retry 机制兜底。

**根因修复**（待办，框架级）：handler 的 status persist 应改为 SSA + 显式 fieldManager（如 `edgion-acme-handler`），由 K8s 的 field ownership 拒绝跨 manager 覆盖。这需要改 `persist_k8s_status` 通用路径，超出本文范围。

## ACME Service

ACME 证书签发由独立的后台 Service 处理：

- **仅 Leader 执行**：证书签发操作仅在 Leader 节点执行，避免多节点重复签发
- **HTTP-01 挑战**：需要 HTTP 端口可达，ACME CA 通过 HTTP 请求验证域名
- **DNS-01 挑战**：通过 DNS 提供商 API 创建 TXT 记录验证域名（支持 Cloudflare、阿里云 DNS）
- **证书存储**：签发成功后创建或更新 Kubernetes Secret，包含证书和私钥
- **自动续期**：监控证书有效期，到期前自动重新签发

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| EdgionAcme → Secret | Secret | `spec.privateKeySecretRef` + `AcmeAccountResolver` | ACME 账户私钥（`accountCredentials`），由 `AcmeService` 首次注册时写入；`Zeroizing<String>` 保护 |
| EdgionAcme → Secret | Secret | `spec.challenge.dns01.credentialRef` | DNS 提供商的 API 凭证（provider-specific key） |
| EdgionAcme → Secret | Secret | 创建/更新（SSA） | 签发的证书（`tls.crt` / `tls.key` / `ca.crt`）写入 `storage.secretName` |
| EdgionAcme ← SecretRefManager | Secret | 级联 requeue | 账户 / DNS 凭证 Secret 变更时重新处理 |
| EdgionAcme → Gateway/EdgionTls | 间接 | 通过证书 Secret | 签发的证书 Secret 被 Gateway/EdgionTls 引用 |
