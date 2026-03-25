---
name: resource-secret
description: Secret 资源：证书/凭证存储、不同步到 Gateway、GLOBAL_SECRET_STORE、级联 requeue。
---

# Secret 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

Secret 是 Kubernetes 核心资源，在 Edgion 中用于存储 TLS 证书、认证凭证、ACME 数据等敏感信息。Secret 属于 **no_sync_kind**，不同步到 Gateway，仅在 Controller 侧处理，通过 GLOBAL_SECRET_STORE 为其他资源提供数据，并通过 SecretRefManager 级联 requeue 依赖资源。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/secret.rs`
- 类型定义: 使用 k8s_openapi 的 `Secret` 类型

## 不同步到 Gateway

Secret 被列入 `DEFAULT_NO_SYNC_KINDS`（`["ReferenceGrant", "Secret"]`），不通过 gRPC 同步到 Gateway。这是安全设计：Secret 的敏感数据仅在 Controller 侧解析后填入引用它的资源（如 Gateway 的 `tls.secrets`、EdgionTls 的 `spec.secret`）。

## Controller 侧处理

### parse

1. **更新 GLOBAL_SECRET_STORE**：将 Secret 数据写入全局存储（`update_secrets`），其他 Handler 的 parse 阶段通过 `get_secret()` 从中读取
2. **初始化阶段累积**：在 init LIST 阶段，Secret 被累积到 `init_accumulator`，init 完成后执行 `replace_all_secrets()` 做一次性全量替换，清除上一次 session 遗留的过期条目
3. **OIDC Secret 校验**：若该 Secret 被 EdgionPlugins 引用，检查是否包含 OIDC 兼容的 key（clientSecret/client_secret/sessionSecret/session_secret/secret），不包含时记录警告

### on_init_done

取出 `init_accumulator` 中累积的所有 Secret，执行 `replace_all_secrets()` 做权威替换。

### on_change

触发级联 requeue：通过 SecretRefManager 获取所有引用此 Secret 的资源，逐一 requeue。这使得 Gateway、EdgionTls、EdgionPlugins、EdgionAcme、BackendTLSPolicy 等资源在 Secret 更新后重新解析。

### on_delete

1. 从 GLOBAL_SECRET_STORE 删除该 Secret
2. 触发级联 requeue（通知依赖资源 Secret 已删除）

## GLOBAL_SECRET_STORE 机制

GLOBAL_SECRET_STORE 是进程级的全局 Secret 缓存：

- 写入时机：SecretHandler 的 parse 阶段
- 读取时机：其他 Handler（GatewayHandler、EdgionTlsHandler、EdgionPluginsHandler 等）的 parse 阶段调用 `get_secret(namespace, name)`
- 生命周期：Controller 进程存活期间持续存在，init_done 时全量替换确保干净状态

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| Secret → Gateway | Gateway | SecretRefManager | Gateway 的 certificateRefs 引用 Secret（TLS 证书） |
| Secret → EdgionTls | EdgionTls | SecretRefManager | EdgionTls 的 secret_ref/ca_secret_ref 引用 Secret |
| Secret → EdgionPlugins | EdgionPlugins | SecretRefManager | JWT/BasicAuth/HMAC/KeyAuth/OIDC 等插件引用 Secret |
| Secret → EdgionAcme | EdgionAcme | SecretRefManager | ACME DNS 凭证引用 Secret |
| Secret → BackendTLSPolicy | BackendTLSPolicy | SecretRefManager | 后端 TLS CA 证书和客户端证书引用 Secret |
