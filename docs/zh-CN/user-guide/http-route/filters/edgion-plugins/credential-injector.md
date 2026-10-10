# CredentialInjector

> **🔌 Edgion 扩展**
>
> `CredentialInjector` 是 `EdgionPlugins` 提供的请求 Filter：它从 Kubernetes Secret 或由控制面（Controller）管理的 `credentialSource` LinkSys 读取上游 Bearer 凭据，并仅将其注入主上游请求；它不属于标准 Gateway API。

## 快速开始

创建一个 `Opaque` Secret，其 `bearerToken` 字段包含原始 token：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: provider-credential
stringData:
  bearerToken: provider-token-value
```

在请求阶段插件中引用该 Secret。省略 `target` 时使用 Bearer 注入，并替换上游 `Authorization` Header：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: provider-credential
spec:
  requestPlugins:
    - type: CredentialInjector
      config:
        credentialRef:
          name: provider-credential
```

上游会收到：

```http
Authorization: Bearer provider-token-value
```

客户端无法覆盖该值：Edgion 会先删除请求中所有与目标同名的 Header，再安排注入已解析的凭据。

## 自定义 Header 目标

对于要求非标准 Header 的提供方，使用 `type: Header`。前缀是静态配置，可以为空。

```yaml
requestPlugins:
  - type: CredentialInjector
    config:
      credentialRef:
        name: provider-credential
      target:
        type: Header
        name: X-Provider-Token
        valuePrefix: "Token "
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|---------|-------------|
| `credentialRef.name` | String | 无 | Secret 或 LinkSys 名称，必填。 |
| `credentialRef.namespace` | String | 插件所在 Namespace | 被引用对象的 Namespace。跨 Namespace 引用需要匹配的 `ReferenceGrant`。 |
| `credentialRef.group` | String | 空字符串（core API group） | 省略或空字符串表示空的 Kubernetes core API group；该 group 支持 kind `Secret`。此 Edgion 字段还允许使用字面值 `core` 引用 Secret。LinkSys 凭据源必须同时指定精确的 group `edgion.io` 和 kind `LinkSys`。 |
| `credentialRef.kind` | String | `Secret` | core group 下的 `Secret`，或在 group 为 `edgion.io` 时显式指定 `LinkSys`。省略 kind 仍表示 `Secret`；不会仅根据 group 推断为 `LinkSys`。 |
| `target.type` | `Bearer` 或 `Header` | `Bearer` | 选择上游 Header 形式。 |
| `target.name` | String | 无 | 仅 `Header` 类型必填。用于路由、分帧、转发、Cookie 或 hop-by-hop 行为的 Header 名称会被拒绝。 |
| `target.valuePrefix` | String | 空 | `Header` 的静态前缀，最长 256 字节。 |

包含前缀在内的完整注入值不得超过 16 KiB。静态 Secret 的类型必须为 `Opaque`（或空类型），必须包含 `bearerToken`，且其内容必须是有效的 UTF-8 Bearer token。凭据缺失、被拒绝、格式错误、被删除或超过大小限制时，资源会设置 `ResolvedRefs=False`；匹配流量会 fail-close，不会在无认证信息的情况下到达上游。

`target.name` 和 `target.valuePrefix` 不会求值模板或 DSL 表达式。需要决定是否执行注入时，使用插件条件；需要动态的非敏感 Header 时，使用 ProxyRewrite 或 DSL。凭据值不会进入通用模板或表达式上下文。

## Controller 管理的 OAuth2

`credentialRef` 可以指向 `type: credentialSource` 的 LinkSys，而不是静态 Secret。Controller 获取 OAuth2 client-credentials token，并将当前的活动值嵌入该插件的 `resolvedCredential`（Admin `/configclient` 会对其脱敏）。数据面（Gateway）不会联系 issuer。

一个 `EdgionPlugins` 资源最多只能包含一个已启用的 LinkSys-backed `CredentialInjector`。基于静态 Secret 的注入器不计入该限制。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: provider-credential
spec:
  requestPlugins:
    - type: CredentialInjector
      config:
        credentialRef:
          group: edgion.io
          kind: LinkSys
          name: provider-oauth
```

### Secret 持久化（默认）

省略 `publication` 或设置 `publication.persist: true` 时，Controller 会创建一个与消费方 `EdgionPlugins` 同名的 Kubernetes Secret。持久化路径仅支持 Kubernetes。在 FS 或 etcd 上，LinkSys 会被拒绝，状态为 `Accepted=False`，reason 为 `UnsupportedConfigurationCenter`。

### 内存发布

在 LinkSys 上设置 `publication.persist: false`，使生成的 token 保存在 Controller 本地内存 map 中，而不是 Secret：

- 不会创建或接管生成的 Secret。
- 允许在单发布者 Controller 上使用（leader-only Kubernetes 或 FS/etcd）。
- Kubernetes `ha_mode: all-serve` 会被拒绝（`Accepted=False`，reason 为 `UnsupportedHaMode`）；Controller 不会调用 issuer。
- 每个 LinkSys 身份（`namespace/name/uid`）仅保存一个活动 token；发布成功时替换该 token，不额外保留上一代用于回退。
- 该 map 仅存在于 Controller：它不会出现在 LinkSys spec、status、conf_sync 或 Admin 字段中。Gateway 仍会通过该插件的 `resolvedCredential` 收到活动 token。

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: provider-oauth
spec:
  type: credentialSource
  config:
    provider:
      type: oauth2ClientCredentials
      tokenEndpoint: https://issuer.example.com/oauth2/token
      clientAuthentication:
        method: clientSecretBasic
        activeSecretRef:
          name: oauth-client
    publication:
      persist: false
```

## 凭据生命周期

Controller 在 `rotation.interval`（默认 `15m`）与 issuer 返回的 token 半寿命中取较早时间，再加少量仅提前的抖动，主动获取下一代 token。消费方的有效期在 issuer 到期时间前预留 30 秒安全余量。获取失败时，当前 token 在其有效期内仍可使用，同时重试获取；如果获取成功但 Secret 发布失败，则保留刚获取的 token 重试发布，避免再次调用 issuer。

`clientAuthentication.previousSecretRef` 是独立的 bootstrap 客户端凭据回退，仅当 issuer 以 `invalid_client` 拒绝活动客户端凭据时使用，不会选择上一代生成的 access token。

切换发布模式时，Edgion 会保留凭据材料，直到替代发布完成：

- `true` 切换到 `false`：Controller 先将活动 token 放入内存，再删除仍与预期 owner 和 generation 匹配的生成 Secret；删除失败会继续重试。
- `false` 切换到 `true`：Controller 为每个当前消费方发布 Secret，并在所有当前发布都一致之前保留内存 token。

删除 `EdgionPlugins` 资源会注销该消费方。仍存在其他消费方时，Controller 会删除已删除消费方的受管 Secret；最后一个消费方消失时，Controller 也会清理 LinkSys 内存 slot。Kubernetes owner reference 为受管 Secret 提供垃圾回收兜底。

内存发布是有意设计的易失存储。丢失领导权、Controller runtime reset 或重启、LinkSys 删除或类型变更、LinkSys UID 替换，以及删除最后一个消费方，都会清空该 slot。凭据源重新激活后，Controller 会重新获取 token。Gateway 可以继续使用已嵌入 PluginRuntime 的 generation，直到 `validUntil`；当 Gateway 没有未过期的已解析凭据时，请求会 fail-close。

## 顺序与隔离

凭据修改遵循普通请求插件的声明顺序：

- 同一 Header 的后一个 `CredentialInjector` 生效。
- 后续的 Header set、append 或 remove 会取消待执行的凭据注入。
- 声明在注入器之前的镜像可能看到客户端的原始 Header，声明在注入器之后的镜像看不到目标 Header。
- 镜像永远不会收到已解析的 Secret 值。
- 一个逻辑请求的重试会保留该请求首次解析的凭据 generation，包括后端级 `ExtensionRef` 插件。

已解析凭据只会在最终、尝试级的主上游 Header 块中物化。它不会写回下游请求，并且会从配置检查、调试输出和插件日志中脱敏。

## Cookie 状态

CredentialInjector 有意不支持结构化 Cookie 注入。Cookie 字段具有合并、重复名称、顺序和引号语义，不能安全地表示为通用 Header 覆盖。在 Edgion 提供独立的结构化 Cookie 模型之前，请使用 Bearer 或专用自定义 Header。

## 相关文档

- [插件组合与引用](../plugin-composition.md)
- [RequestMirror](./request-mirror.md)
