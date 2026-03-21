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

### validate

严格校验配置完整性：

1. **email**：必填，用于 ACME 账户注册
2. **domains**：至少一个域名
3. **通配符域名校验**：`*.example.com` 格式的通配符域名必须使用 DNS-01 挑战类型
4. **挑战配置完整性**：
   - HTTP-01 模式：必须提供 http01 配置
   - DNS-01 模式：必须提供 dns01 配置，包括：
     - provider 名称校验（支持 cloudflare、alidns）
     - DNS 凭证 Secret 存在性检查（不存在则警告）

### parse

1. **清除旧引用**：清除 SecretRefManager 中该 EdgionAcme 的旧记录
2. **解析 DNS 凭证 Secret**（仅 DNS-01 模式）：
   - 从 `dns01.credential_ref` 获取 Secret 引用
   - 在 SecretRefManager 注册引用关系
   - 从 GLOBAL_SECRET_STORE 读取 Secret 数据，填入 `spec.dns_credential_secret`
3. **通知 ACME Service**：调用 `notify_resource_changed()` 通知后台 ACME Service 有资源变更，触发证书签发/续期流程

### on_delete

清除 SecretRefManager 引用。

### update_status

- 若有 validation_errors，设置 `last_failure_reason`
- Status 包含 ACME 生命周期状态（phase、证书信息、最后成功/失败时间等）

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
| EdgionAcme → Secret | Secret | dns01.credential_ref | DNS 提供商的 API 凭证 |
| EdgionAcme → Secret | Secret | 创建/更新 | 签发的证书存入 Secret |
| EdgionAcme ← SecretRefManager | Secret | 级联 requeue | DNS 凭证 Secret 变更时重新处理 |
| EdgionAcme → Gateway/EdgionTls | 间接 | 通过 Secret | 签发的证书 Secret 被 Gateway/EdgionTls 引用 |
