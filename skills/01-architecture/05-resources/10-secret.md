---
name: resource-secret
description: Secret 资源：存储、不同步到 Gateway、GLOBAL_SECRET_STORE、级联 requeue；以及 Controller/Gateway 共用的 Secret → Material 解析层。
---

# Secret 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

Secret 是 Kubernetes 核心资源，在 Edgion 中用于存储 TLS 证书、认证凭证、ACME 账户等敏感信息。Secret 属于 **no_sync_kind**，不通过 gRPC 同步到 Gateway，仅在 Controller 侧处理：一方面通过 GLOBAL_SECRET_STORE 为其他 Handler 供数，另一方面通过 `SecretRefManager` 级联 requeue 依赖资源。需要让 Gateway 用到的敏感数据，由 Controller 在 parse 阶段**内嵌**到引用它的资源 spec 中（如 `Gateway.tls.secrets`、`EdgionTls.spec.secret`、`LinkSys.spec.config.<auth>.secret` / `tls.certs.secret`），随 conf_sync 一起传过去。EdgionAcme 是例外：账户 / DNS 凭证由 Controller 侧的 `AcmeService` 使用，不内嵌到 spec；签发期间 `AcmeService` 通过 K8s API 直接拉取 Secret。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/secret.rs`
- 凭据解析层（Controller + Gateway 共用）: `src/types/credentials/`
- Controller 端 credential pipeline: `src/core/controller/conf_mgr/sync_runtime/resource_processor/credential/`
- Gateway 端凭据四态: `src/core/gateway/link_sys/credential.rs`

## Controller 侧处理

### parse

1. **更新 GLOBAL_SECRET_STORE**：写入全局存储（`update_secrets`），其他 Handler 的 parse 阶段通过 `get_secret()` 读取。
2. **初始化阶段累积**：init LIST 阶段 Secret 先进 `init_accumulator`，init_done 时 `replace_all_secrets()` 一次性全量替换，清理上个 session 的残留。
3. **OIDC Secret 校验**：若被 EdgionPlugins 引用，检查是否含 OIDC 兼容 key（`clientSecret` / `client_secret` / `sessionSecret` / `session_secret` / `secret`），缺失则警告。

### on_init_done / on_change / on_delete

- `on_init_done`：执行 `replace_all_secrets()` 权威替换。
- `on_change`：通过 `SecretRefManager` 找到所有引用此 Secret 的资源逐一 requeue。
- `on_delete`：从 GLOBAL_SECRET_STORE 删除 + 级联 requeue。

## GLOBAL_SECRET_STORE 机制

进程级全局缓存：SecretHandler.parse 时写入，其他 Handler（Gateway / EdgionTls / EdgionPlugins 等）parse 时通过 `get_secret(ns, name)` 读取。生命周期跟随 Controller 进程，init_done 时全量替换保证干净状态。

## Secret → Material 解析层

`src/types/credentials/` 下的 `SecretMaterialResolver` trait 把 `Secret.data`（`BTreeMap<String, ByteString>`）映射成强类型 `Material`。Controller 与 Gateway **共用同一套 resolver**，避免两端重复实现。

### trait

```rust
pub trait SecretMaterialResolver: Send + Sync {
    type Material: Send + Sync;
    fn required_keys(&self) -> &'static [&'static str];
    fn allowed_secret_types(&self) -> &'static [&'static str] { &["Opaque", ""] }   // 默认
    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError>;
}
```

`ResolveError`：`MissingKeys(Vec<&'static str>)`（缺键一次性全报）/ `DecodeError { key, source }` / `InvalidSecretType { got, allowed }`（Secret.type 不在白名单，由 pipeline 校验）。

### Resolver 全表

| Resolver | required_keys（Secret.data） | allowed_secret_types | Material |
|----------|------------------------------|----------------------|----------|
| `BasicAuthResolver` | `password` 必须，`username` 可选 | `kubernetes.io/basic-auth`, `Opaque`, `""` | `BasicAuthMaterial` |
| `BearerResolver` | `bearerToken` | `Opaque`, `""` | `BearerMaterial` |
| `HmacResolver` | `hmacKey` | `Opaque`, `""` | `HmacMaterial` |
| `EsApiKeyResolver` | `apiKeyId` + `apiKeySecret` | `Opaque`, `""` | `EsApiKeyMaterial` |
| `AcmeAccountResolver` | `accountCredentials`（JSON） | `Opaque`, `""` | `AcmeAccountMaterial` |
| `TlsCertsResolver` | `tls.crt` / `tls.key` / `ca.crt` 至少一个（三键全列进 `required_keys()` 是为了 status 打印完整候选集） | `kubernetes.io/tls`, `Opaque`, `""` | `TlsMaterial` |

应用层键名用 camelCase；TLS 键沿用 `kubernetes.io/tls` 标准。

### 哪些资源走这套 resolver

| 资源 | 字段 | Resolver |
|------|------|----------|
| `LinkSys` (Redis / Etcd) | `spec.config.auth.secretRef` | `BasicAuthResolver` |
| `LinkSys` (Elasticsearch) | `spec.config.auth`（tagged enum） | `BasicAuth` / `EsApiKey` / `Bearer` |
| `LinkSys` (Webhook) | `spec.config.auth`（tagged enum） | `BasicAuth` / `Bearer` / `Hmac` |
| `LinkSys`（所有 provider） | `spec.config.tls.certs.secretRef` | `TlsCertsResolver` |
| `EdgionAcme` | `spec.privateKeySecretRef` | `AcmeAccountResolver` |

> LinkSys / EdgionAcme 的 `*Auth` / `TlsCerts` 都启用了 `deny_unknown_fields`，CRD 里内联 `username` / `password` / `bearerToken` / `apiKeyId` / `clientKey` 等敏感字段会在 YAML 反序列化阶段直接报错。`caCert` / `clientCert` 是公开材料，仍可内联。

### Controller → Gateway 职责分界

```
┌─────────────────────────────┐   conf_sync (gRPC)   ┌──────────────────────────────┐
│ Controller                  │ ─────────────────►  │ Gateway                      │
│ 1. SecretRefManager 登记    │                     │ 4. 从内嵌的 Secret.data 再次  │
│ 2. pipeline.resolve_secret： │                     │    调 resolver.parse 得       │
│    ReferenceGrant 校验 →    │                     │    Material，封装成           │
│    SecretStore 查找 →        │                     │    CredentialState<M>         │
│    resolver.parse（只校验）  │                     │ 5. Resolved/NotRequired →     │
│    OK → 内嵌整份 Secret 到   │                     │    build provider；Pending/   │
│    *.auth.secret / tls.secret│                     │    Invalid → fail-fast bail!  │
│ 3. 写 Accepted + ResolvedRefs│                     │    （不退化为匿名连接）        │
└─────────────────────────────┘                     └──────────────────────────────┘
```

- **Controller**（`resource_processor::credential::pipeline`）：命名空间 + `ReferenceGrant` 授权、`SecretStore` 查找、`resolver.parse` 做缺键 / 编码 / Secret.type 校验（Material 丢弃）。三态 `ResolveOutcome` 映射到 `ResolvedRefs` condition：`Ok`→True；`Pending`→False/`SecretNotFound`/`RefNotPermitted`（等 requeue）；`Invalid`→False/`MissingKeys`/`DecodeError`/`InvalidSecretType`（需人工修 Secret）。
- **Gateway**（`core::gateway::link_sys::credential`）：从 spec 内嵌 `Secret` 再算一次 Material。`CredentialState<M>` 四态 `NotRequired` / `Pending` / `Invalid` / `Resolved`，`Debug` 对 `Resolved` 变体脱敏，避免 `tracing::info!(state=?)` 泄密码。Dispatch 只在 `NotRequired` / `Resolved` 下构建 client，**不会**退化为空凭据的"匿名连接"（历史 bug）。

### ResolvedRefs 多错误收集（强制规约）

> 适用范围：所有在单次 parse 中解析**多个** Secret / 资源 ref 的 Handler。
> 目前落地：`LinkSysHandler`、`EdgionPluginsHandler`、HTTPRoute 系列（via `route_utils::collect_ref_not_permitted_errors`）。

**规则**：Handler 在解析多个 ref 时，必须把所有失败收集到 `Vec<ResolvedRefsError>` 再一次性喂给 `resolved_refs_condition`，**不得在第一个 `Err` 上 `return`**。

```rust
async fn resolve_for_spec(...) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    if let Some(auth) = &mut r.auth {
        if let Err(e) = resolve_one(ctx, owner, &auth.secret_ref, &BasicAuthResolver).await {
            errors.push(e);
            // ❌ 不要 `return errors;` —— 继续走 tls
        } else { /* 内嵌 Secret */ }
    }
    if let Some(e) = resolve_tls(ctx, owner, r.tls.as_mut()).await {
        errors.push(e);
    }
    errors
}
```

**为什么强制**：

1. **Gateway API message 契约**：`status.conditions[ResolvedRefs].message` 是聚合消息，一个 LinkSys 同时挂错了 auth 和 tls 的 Secret 时，操作员必须在一次 reconcile 里看到全部错误，否则"修完 auth、重新 reconcile、发现 tls 还错、再改"会多绕一轮。
2. **`pick_resolved_refs_reason` 优先级表才能生效**：`status_utils::pick_resolved_refs_reason` 按严重度给 reason 排序（`InvalidKind` > `RefNotPermitted` > `InvalidSecretType` > `PluginMetaDataNotFound` > `BackendNotFound` > `SecretNotFound` > `DecodeError` > `MissingKeys`）。fail-fast 每次只产出一个 error，优先级表形同虚设。
3. **半成品 spec 安全**：失败分支留 `auth.secret: None` / `tls.certs.secret: None` 不会被消费，因为 Gateway 端只在 `ResolvedRefs=True` 时才 `build_link_sys_instance_from_status` → build client（见 I4/I5、`CredentialState` 四态）。

**新增 Handler / 新增字段时**：任何在 `resolve_for_spec` 类函数里多点解析 ref 的，写一条 `test_<foo>_and_<bar>_both_fail_collects_both_errors` 单测断言 `errors.len() == 2` + picked reason 正确 + message 包含两段，防止后续同学不小心改回 fail-fast。历史参考：`tasks/fix-change/M7-linksys-multi-error-collect.md`（LinkSys 从 fail-fast 摘除的完整改动）。

### Zeroize scope（defense-in-depth，非端到端）

只有**持有原始私钥**的 Material 字段被 `Zeroizing` 包裹：
- `AcmeAccountMaterial.credentials`（`Zeroizing<String>`）
- `TlsMaterial.client_key_pem`（`Option<Zeroizing<Vec<u8>>>`）
- 其余（Basic / Bearer / Hmac / EsApiKey）裸 `String`，对齐 cert-manager。

以下明文副本**不擦除**：`SecretStore` 缓存、`serde_json` 序列化 buffer、conf_sync gRPC 传输、Gateway runtime store 缓存、`AcmeAccountResolver::parse` 里 `serde_json::from_str::<Value>` 的 tree、`json.to_string()` 等中间 String。上游 K8s apiserver 响应 buffer 本身就无法擦除，做端到端擦除收益有限，因此本层仅保证"最终材料容器 zeroize"。详情见 `materials.rs` 模块头 caveat 以及 `resolvers/acme.rs` 的 NOTE。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| Secret → Gateway | Gateway | SecretRefManager | `certificateRefs`（TLS 证书） |
| Secret → EdgionTls | EdgionTls | SecretRefManager | `secret_ref` / `ca_secret_ref` |
| Secret → EdgionPlugins | EdgionPlugins | SecretRefManager | JWT / BasicAuth / HMAC / KeyAuth / OIDC 等 |
| Secret → EdgionAcme | EdgionAcme | SecretRefManager | `privateKeySecretRef`（账户私钥，`AcmeAccountResolver`）+ `dns01.credentialRef`（DNS Provider API） |
| Secret → LinkSys | LinkSys | SecretRefManager | `auth.secretRef`（basic/apiKey/bearer/hmac）+ `tls.certs.secretRef`（mTLS 私钥） |
| Secret → BackendTLSPolicy | BackendTLSPolicy | SecretRefManager | 后端 TLS CA 与客户端证书 |
