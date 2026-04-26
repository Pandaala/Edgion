---
name: resource-link-sys
description: LinkSys 资源：外部系统连接器定义、多种 Provider 类型、Gateway 侧客户端管理、统一 secretRef 凭据解析。
---

# LinkSys 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)
> **凭据解析层**：参见 [10-secret.md](10-secret.md) 的 "Secret → Material 解析层" 小节
> **外部契约 / YAML Schema**：参见 [../../02-features/03-resources/18-link-sys.md](../../02-features/03-resources/18-link-sys.md)

LinkSys 定义外部系统连接器（Redis / Etcd / Elasticsearch / Webhook / Kafka），供 AccessLog、限流插件、Webhook Key 解析等功能使用。所有凭据通过 `secretRef` 引用 K8s `Secret`，**CRD 里不接受任何内联敏感字段**（`deny_unknown_fields` 硬拒）。

## 源码位置

| 作用 | 路径 |
|------|------|
| Controller Handler | `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs` |
| Credential pipeline（Controller） | `src/core/controller/conf_mgr/sync_runtime/resource_processor/credential/` |
| 资源类型定义 | `src/types/resources/link_sys/`（mod.rs、redis.rs、etcd.rs、elasticsearch.rs、webhook.rs、common.rs） |
| 共享 SecretObjectReference / SecretSlot / TLS 类型 | `src/types/resources/common/{secret_object_ref,secret_slot,tls}.rs` |
| 凭据解析层（Controller + Gateway 共用） | `src/types/credentials/`（materials / resolver / resolvers） |
| Gateway 凭据四态 | `src/core/gateway/link_sys/credential.rs` |
| Gateway TLS 组装 | `src/core/gateway/link_sys/tls.rs` |
| Gateway 运行时 Store | `src/core/gateway/link_sys/runtime/store.rs` |
| Provider 客户端 | `src/core/gateway/link_sys/providers/{redis,etcd,elasticsearch}/` |

## 推荐 `use` 路径（避免命名混淆）

LinkSys CRD 与日志输出/轮转配置（用于 link_sys 运行时写日志）虽然名字相近，但是两套完全不同的类型：

| 你想要的东西 | 推荐 `use` 路径 |
|------|------|
| LinkSys CRD 结构体（Redis/Etcd/ES/Webhook 连接定义） | `use crate::types::{LinkSys, LinkSysSpec};`（顶层重导出）<br>或 `use crate::types::resources::link_sys::*;`（直接路径） |
| 日志输出 / 文件轮转配置（`StringOutput`、`RotationConfig`、`LocalFileWriterCfg`） | `use crate::types::output::*;`（**唯一路径**，过去的 `crate::types::link_sys` 别名已删除） |
| link_sys 运行时（`DataSender` trait、provider 客户端） | `use crate::core::gateway::link_sys::*;` |

`crate::types::link_sys` 这个模块路径**不再存在**。如果还看到这种写法，是历史遗留的别名残留，统一改为上表中的路径。

## CRD 顶层布局

`LinkSysSpec` 用 `#[serde(tag = "type", content = "config")]` tagged enum：
- 顶层只有两个字段：`spec.type` 和 `spec.config`
- `config` 类型由 `type` 决定：`redis` → `RedisClientConfig`、`etcd` → `EtcdClientConfig`、`elasticsearch` → `ElasticsearchClientConfig`、`webhook` → `WebhookServiceConfig`、`kafka` → `KafkaClientConfig`（占位）
- 旧 `spec.config.type` / `spec.config.config` 嵌套写法已不再被接受

`SecretObjectReference`（`src/types/resources/common/secret_object_ref.rs`）、`SecretSlot`（`src/types/resources/common/secret_slot.rs`）与 `TlsConfig` / `TlsCerts`（`src/types/resources/common/tls.rs`）已从各 provider 模块抽公共，避免多份近似拷贝。`SecretObjectReference` 含 `group / kind / name / namespace` 四字段（前两者可选，缺省即 `core/Secret`），为将来扩展非 Secret 凭据源（Vault / ExternalSecret）保留；`SecretSlot` 是 `{ secret_ref: SecretObjectReference, secret: Option<Secret> }` 的组合槽位，通过 `#[serde(flatten)]` 嵌入各 `*Auth`，YAML 形态不变。LinkSys 各子 crate 通过 `pub use` 保持路径稳定。

## Controller 侧处理

### preparse / validate

- 调 `LinkSys::validate_config()`：
  - Redis/Etcd/Elasticsearch `endpoints` 为空 → error
  - Redis `topology.mode=sentinel` 但无 `sentinel` 子对象 / `mode=cluster` 但无 `cluster` 子对象 → error
  - Webhook：URI 非法、method 非法、`statusOnError` 越界、`maxResponseBytes` 超上限 → error
- 未实现的 provider（如 `Kafka`）**不**产生 error，保持 `Accepted=True`（schema 合法，只是后端尚未接入，避免升级时把已有资源打成 Invalid）。

### parse — 凭据解析

与旧版"直接透传"不同，新版 parse 会走完整的 **credential pipeline**（`resource_processor/credential/pipeline.rs`）：

1. 通过 `AuthSpec` / `TlsSpec` trait + `drive_standard` 泛型分派，统一遍历 spec 里的所有 `SecretObjectReference`：
   - `AuthSpec` trait 把"声明 → resolver 选择 → material 提取"绑定到具体 `*Auth` 类型（Redis/Etcd 的 `RedisAuth`/`EtcdAuth`；ES/Webhook 的 tagged enum 各 variant）
   - `TlsSpec` trait 处理所有 provider 可选的 `tls.certs.secretRef`（`TlsConfig` 已实现此 trait，直接复用）
   - `resolve_for_spec` 调用 `drive_standard::<A: AuthSpec, T: TlsSpec>` 折叠所有 ref 的处理
   - 新增后端只需：(1) impl `AuthSpec`；(2) 加 `SystemConfig` 变体；(3) `resolve_for_spec` 加一行 `drive_standard(...)`；(4) 如有 TLS，`TlsConfig` 已 impl `TlsSpec` 直接复用
2. 对每个 ref 执行七步 pipeline：
   1. 计算 effective namespace（缺省 = LinkSys 所在 ns）
   2. 在 `SecretRefManager` 登记 `owner→secret` 依赖（即使后续步骤失败也要登记，这样 Secret 出现/变更时能 requeue 自愈）
   3. 跨 ns 则过 `ReferenceGrantStore` 授权
   4. 从 `runtime.secret_store` 查 `Secret`
   5. 校验 `Secret.type` 在该 resolver 的白名单内
   6. 取出 `Secret.data`
   7. 调对应 `SecretMaterialResolver::parse` 做缺键 / 编码校验（Material 丢弃；Gateway 会再算一次）
3. 成功则把**整份 `k8s_openapi::Secret` 对象**写回 spec 的 `auth.secret` / `tls.certs.secret` 字段（不是只写 data；保留 type / metadata 以便 Gateway 做 `allowed_secret_types` 二次校验）。这两个字段在 CRD schema 上有 `skip_serializing_if = "Option::is_none"`，用户的 YAML 不会出现，同时又能随 conf_sync 的 `serde_json` 序列化到 Gateway。
4. Pipeline 三态 `ResolveOutcome`（仅作用于**当前这个 ref**，不影响同一个 LinkSys 里其它 ref 继续走 pipeline —— 见 [10-secret.md](10-secret.md) "ResolvedRefs 多错误收集"）：
   - `Ok(Secret)` → 内嵌 Secret 到 spec
   - `Pending(SecretNotFound | ReferenceGrantMissing)` → 当前 ref 留 `secret: None`，错误累加；等 Secret / Grant 到位后自动 requeue
   - `Invalid(MissingKeys | DecodeError | InvalidSecretType)` → 当前 ref 留 `secret: None`，错误累加；等运维修 Secret 内容
   - 无论命中哪个分支，`resolve_for_spec` 都会继续处理下一个 ref（auth 失败不短路 tls，反之亦然），全部结果一并写进 `ResolvedRefs.message`；`reason` 按 `pick_resolved_refs_reason` 优先级表取最严重那个。

### update_status

Handler 同时维护两个 Gateway-API 风格的 condition：

| Condition | True | False 的 reason |
|-----------|------|----------------|
| `Accepted` | spec 合法 + `validate_config()` 通过 | `Invalid`（validation error 列表） |
| `ResolvedRefs` | 全部 secretRef / tls.secretRef 都已 `Ok` | `SecretNotFound` / `RefNotPermitted` / `InvalidSecretType` / `MissingKeys` / `DecodeError` |

`SecretNotFound` / `RefNotPermitted` 是 **Pending** 语义（等自愈），其余 `Invalid` 语义（等人改）。Controller 通过 `SecretRefManager` / `ReferenceGrantStore` 的变更事件自动 requeue，Handler 不用手动重试。

`kubectl get linksys` 输出 `Type` / `Endpoints` / `Accepted` / `ResolvedRefs` / `Age` 五列（见 CRD `additionalPrinterColumns`）。

## Gateway 侧处理

### conf_sync 收到 spec 后

`LinkSysStore` 负责 provider 客户端的生命周期：
- LinkSys 创建 → 构建 provider client
- LinkSys 更新 → 对照 `config_mapping` 决定是否需要重建 client（Redis 参数变、endpoint 变、auth 变、tls 变 → 重建；observability 变 → 只改 hot 字段）
- LinkSys 删除 → 优雅关闭连接 / 放回连接池

### 凭据二次解析：`CredentialState`

Gateway 拿到的 spec 里，`auth.secret` / `tls.certs.secret` 已经有内嵌 `Secret` 对象。`core::gateway::link_sys::credential` 模块把凭据抽象成四态：

```rust
enum CredentialState<M> {
    NotRequired,     // CRD 里根本没配 auth → provider 用匿名连接
    Pending,         // 配了 secretRef 但 Controller 还没把 secret 填进来
    Invalid(reason), // secret 填进来了但 resolver.parse 失败
    Resolved(M),     // 可用的 Material
}
```

约束：
- `Debug` 实现**脱敏**：`Resolved` 变体不打印 Material；这样 `tracing::info!(state=?state)` 不会泄密码。
- `is_buildable()` 只在 `NotRequired` / `Resolved` 返回 true。Provider 的 dispatch 路径做 fail-fast：`Pending` / `Invalid` 直接 `bail!`，**不会**退化到"空凭据 / 匿名连接"（历史上的静默降级 bug）。
- 对 tagged-enum 的 auth（ES / Webhook），用通用 `CredentialState<ResolvedCredential>` 包起来，`ResolvedCredential` 再按 variant 分发。

### TLS 组装

`src/core/gateway/link_sys/tls.rs` 负责把 `TlsConfig` + 解析出的 `TlsMaterial` 塞到各 provider 的 TLS 配置：
- `ca_pem`：若 `TlsCerts.caCert` inline 存在，用 inline；否则用 material 的 `ca_pem`。
- `client_cert_pem` / `client_key_pem`：优先 material（走 secretRef），私钥是 `Zeroizing<Vec<u8>>`。
- `insecureSkipVerify` 按 provider 能力处理：etcd 忽略并打 warning；Redis/ES/Webhook 遵循。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| LinkSys → Secret | `Secret` | `SecretRefManager`（auth + tls） | Secret 变更/删除 → 级联 requeue 本 LinkSys |
| LinkSys → ReferenceGrant | `ReferenceGrant` | `ReferenceGrantStore` | 跨 ns secretRef 授权；Grant 变更 → 级联 requeue |
| LinkSys ← AccessLog | AccessLog 配置 | 按 `namespace/name` 引用 | ES / Redis / Webhook 作为日志后端 |
| LinkSys ← EdgionPlugins | 插件配置 | 按 `namespace/name` 引用 | Redis 分布式限流、Webhook Key 解析 |

内部依赖与 requeue 级联全图见 [19-resource-dependency.md](19-resource-dependency.md)。

## 相关任务 / 设计 gap

- Zeroize scope 边界（为什么内嵌 Secret 走 `serde_json`/gRPC 时不做端到端擦除）见 [10-secret.md](10-secret.md) 的 "Zeroize scope" 小节。
- Provider 客户端重建策略详见 `providers/*/config_mapping.rs`。
