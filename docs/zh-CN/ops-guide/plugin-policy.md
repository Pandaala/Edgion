# 插件策略（允许清单 / 拒绝清单）

Controller 可以通过可选配置 `validation.plugin_policy` 限制 Gateway 允许执行哪些插件类型（allowlist 或 denylist）。被限制的插件永远不会被 Gateway 实例化；根据配置的动作，依赖该插件的请求要么以可配置的状态码被**阻断（block）**，要么在**跳过该插件后放行（bypass）**——两种情况都是可观测的：访问日志、Gateway 指标、资源状态条件，以及 Controller 异常资源指标都会记录。

本文档是运维指南：策略做了什么、如何安全地配置它，以及启用前必须阅读的警告。字段级配置 schema 请参见
[Controller 配置 — validation.plugin_policy](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/02-config/00-controller-config.md#validationplugin_policy)。

## 工作原理

- **Controller（识别 + 上报）**：在启动时验证策略（任何错误均导致启动失败），在 preparse 阶段为匹配的插件条目打上内部标记，并上报专用的 `PluginPolicyRestricted` 状态条件，列出命中项。它**不会**翻转条目的 `enable` 字段，也**不会**阻止资源被缓存或同步到 Gateway。
- **Gateway（执行 + 观测）**：被打标的条目永远不会被实例化（不初始化 Wasm VM、不建立外部连接等）。`Block` 条目会拒绝请求；`Bypass` 条目会被跳过，请求在没有该插件的情况下继续执行。

策略缺省（默认状态）意味着允许所有插件：每个已编译的插件类型对所有资源仍然可用，没有条目被打标，没有条目状态条件被上报，Gateway 沿用现有的零开销代码路径。只有当运维人员显式添加下面的 `plugin_policy` 配置块时，该特性才会生效。

## 配置

```yaml
validation:
  plugin_policy:
    mode: denylist       # denylist | allowlist
    plugins:
      - name: http/Wasm  # <family>/<Type>；简写形式使用 default_action
```

仅支持配置文件（无对应 CLI 参数）。**策略变更需要重启 Controller**——本配置段没有热重载。

条目命名格式为 `<family>/<Type>`，覆盖三个 family（`http`、`stream`、`tls-route`）。`denylist` 仅限制清单中列出的插件；`allowlist` 会限制所有未列出的插件，跨三个 family **全局**生效——没有按 family 的豁免。Controller 会在启动时校验策略，遇到非法条目（未知 family/类型、重复条目、超出范围的 `block_status`，或在 `allowlist` 条目上设置了 `action`/`block_status`）会拒绝启动。完整字段参考、限定名表格以及完整的 allowlist/denylist 语义，请参见
[Controller 配置 — validation.plugin_policy](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/02-config/00-controller-config.md#validationplugin_policy)。

## 运维人员可观测的内容

### 资源状态

当某个资源中至少有一个条目被打标时，该资源会获得一个专用状态条件：

```yaml
status:
  conditions:
    - type: PluginPolicyRestricted
      status: "True"
      reason: RestrictedByPolicy
      message: "requestPlugins[2] http/Wasm -> block(503); plugins[0] stream/ConnectionRateLimit -> bypass"
```

`Accepted` 不受此影响——它仍然表示"资源有效且已同步"。当策略被放宽（或移除）并且 Controller 重启后，下一次 preparse 会主动移除陈旧的 `PluginPolicyRestricted` 条件，而不是让它一直卡住。

### Controller 指标

`edgion_controller_resource_abnormal` 将 `PluginPolicyRestricted:True` 归类为 **err** 级触发项——`block` 和 `bypass` 命中都算，因为任何被限制插件的使用都超出预期，无论采取何种动作。此指标的可见性**仅限 Kubernetes 模式**：它依赖资源的实时状态经过 watch 往返回传。在 FileSystem/Etcd 模式下，扫描器读取的是预处理对象，永远看不到状态条件（这是该指标本身已有的特性，并非插件策略专属）。

### Gateway 行为 — HTTP

- **Block**：请求以配置的状态码被拒绝，响应体是固定的通用内容 `{"message":"plugin disabled by policy"}`——插件名称永远不会发送给客户端。阻断闸门位于请求阶段，会短路整条插件链，无论被限制的插件实际配置在哪个阶段：一个依赖被阻断的响应阶段插件的路由，会在入口处被 fail closed，而不是在浪费一次上游往返之后才被拒绝。当同一条链中有多个条目被阻断时，按 spec 顺序（各阶段依次为 `requestPlugins`、`upstreamResponseFilterPlugins`、`upstreamResponseBodyFilterPlugins`、`upstreamResponsePlugins`，阶段内按列表顺序）排在最前的条目决定返回的状态码。
- **Bypass**：该插件被跳过；插件链照常继续执行。
- 访问日志：被阻断的请求会带有 `PluginPolicyBlock` 插件日志条目，记录限定插件名；被跳过的请求会带有 `PluginPolicyBypass` 条目，列出被跳过的插件。没有命中就没有对应条目。
- 指标：`edgion_gateway_plugin_policy_blocked_total` 与 `edgion_gateway_plugin_policy_bypassed_total`，均带有限定插件名标签。

### Gateway 行为 — Stream / TLS-route

- **Block**：连接被关闭。由于在 TLS 握手前或流中途都无法发送 HTTP 状态码，`stream/` 与 `tls-route/` 条目的 `block_status` 会被忽略。
- **Bypass**：插件被跳过；流日志会记录该标记，资源的 `PluginPolicyRestricted` 条件提供资源级可见性。

## 启用前必读的运维警告

1. **绕过（bypass）认证/鉴权类插件会让相关路由变得更不严格，而不是更严格。** 如果某条路由的安全态势依赖某个插件（例如 `JwtAuth`、`BasicAuth`、`IpRestriction`、`KeyAuth`），请使用 `action: block` 限制它，而不是 `bypass`——绕过一个鉴权插件会让原本应被拒绝的请求未经认证就到达后端。
2. **这只是执行控制，不是代码移除。** 限制某个插件类型不会从 Gateway 二进制文件中移除其代码或依赖——它只是阻止该插件实例被实例化和运行。不要把 `plugin_policy` 当作"编译时不包含某插件"的替代方案。
3. **升级顺序很重要。** 在 Controller 上启用 `plugin_policy` **之前**，先将新版本 Gateway 二进制文件（能识别内部策略标记的版本）部署到集群中**所有** Gateway 上。旧版本 Gateway 不认识该标记字段——serde 会静默跳过未知键——并照常运行该插件。这是**fail-open（失败开放）**：你以为在所有 Gateway 上都被阻断的插件，实际上可能仍在尚未升级的 Gateway 上不受限制地运行。
4. **该策略不覆盖原生 `HTTPRoute` 过滤器。** 它仅管辖通过 `EdgionPlugins` / `EdgionStreamPlugins` 配置的插件。例如被你以 `http/RequestRedirect` 拒绝的过滤器，仍然可以通过原生 Gateway API `HTTPRoute` 过滤器完整表达——该策略无法（也不会）限制这条路径。如果你需要彻底禁止某种行为，仅在此处拒绝它是不够的。
5. **`block` 闸门是无条件的。** 即使该请求本应满足插件的逐条目 `conditions`（例如时间窗口条件）而被跳过，它仍然会被拒绝。如果你依赖时间窗口或其他条件化的插件执行，请注意：对该插件类型的策略阻断会完全覆盖条件判断——无论该插件对这个具体请求本应是否运行，整条链都会被拒绝。

## 相关文档

- [Controller 配置 — validation.plugin_policy](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/02-config/00-controller-config.md#validationplugin_policy) — 完整字段参考。
- [EdgionPlugins — HTTP 插件](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/13-edgion-plugins.md) — HTTP 插件目录（`http/` family 名称）。
- [EdgionStreamPlugins — Stream 插件](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/14-edgion-stream-plugins.md) — stream/TLS-route 插件目录（`stream/` 与 `tls-route/` family 名称）。
