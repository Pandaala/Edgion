# 染色（Dye，流量标记请求头）

`dye` 是一种条目级（entry-level）机制，用于根据插件实际做了什么——选中了哪个目标，或触发了哪种失败行为——给请求或响应打上运维自定义的 HTTP 头标记。它本身不是一个插件，而是任何支持它的插件条目上与 `type`/`config`/`alias`/`conditions` 同级的一个字段。

本页是 `dye` 的唯一权威参考文档。各插件页面（[Canary](./canary.md)、[Dynamic Upstream](./dynamic-upstream.md)、[Direct Endpoint](./direct-endpoint.md)、[RegionRoute](./region-route.md)，以及依赖失败类插件）均链接回本页获取共享的术语和语义，各自页面只保留该插件特有的内容（value 的来源、协议限定、以及该插件实际会记录哪些结果）。

## 结构（Shape）

```yaml
- type: Canary
  enable: true
  dye:
    request:
      - name: X-Release-Track          # 不设置 on => [success]；不设置 value => 插件记录的值
    response:
      - name: X-Edgion-Outcome
        on: [fallback, reject]
      - name: X-Edgion-Degraded
        on: [fail-open, fail-close]
        value: degraded                # 运维自定义的静态值覆盖
        conditions:                    # 可选；与条目级 conditions 复用同一套 PluginConditions 结构
          run:
            allOf:
              - type: keyExist
                key: { type: header, name: X-Debug-Token }
      - name: X-Edgion-Debug
        on: [fallback]
        value: "fallback for ${header:X-Account-Id}"   # 基于共享 ${type:name} 引擎的模板
  config:
    # ... 插件配置；dye 不嵌套在 config 之下 ...
```

`DyeSuite { request: [DyeRule], response: [DyeRule] }`；
`DyeRule { name, on: [DyeOutcome]（默认 [success]）, value?, conditions? }`。

## 结果词表（`on`）

| 结果 | 含义 | 写入语义 |
|---|---|---|
| `success` | 插件成功解析并使用了某个目标/选择结果。 | Set（整体替换） |
| `fallback` | 选择类插件回退到普通路由，而非自身的选择结果。 | Append（不去重） |
| `reject` | 选择类插件拒绝了请求，而非完成选择。 | Append（不去重） |
| `fail-open` | 依赖失败类插件在其依赖失败时仍放行了请求。 | Append（不去重） |
| `fail-close` | 依赖失败类插件因依赖失败而拒绝了请求。 | Append（不去重） |

一条规则的 `on` 列表可以包含以上结果的任意组合，**唯一的例外**是不能在同一条规则里混合 `success` 与任意失败结果（`fallback`/`reject`/`fail-open`/`fail-close`）——这两类结果的 value 来源和写入语义（Set 与 Append）都不同，一条规则无法同时服务两者。如果希望同一个请求头既有成功值又有失败值，用两条同名的规则即可——只要这两条规则不在同一个资源内使用相同的 `name`（见下方的 Set-clobber 校验），失败 Append 就不会覆盖之前的成功 Set。

**方向**：`request` 方向的规则在 v1 中只能配置 `on: [success]`——请求方向的失败染色（向上游暴露降级状态）是文档化的未来扩展，目前尚不支持。失败结果只能配置在 `response` 方向的规则上。

## 能力表——哪些插件类型能记录哪些结果

配置一个插件类型从不记录的结果是准入错误（在 Controller 与 Gateway 编译阶段均会拒绝），而不是一个静默永不出现的请求头：

| 插件 | 可染色的结果 |
|---|---|
| `Canary`、`DynamicInternalUpstream`、`DynamicExternalUpstream`、`DirectEndpoint` | `success`、`fallback`、`reject` |
| `RegionRoute` | `success`、`fallback`（无 `reject`——RegionRoute 在 v1 中没有 `onMissing`/`onNoMatch`/`onInvalid` 这类会走到 reject 的配置路径） |
| `RateLimitRedis`、`ExtProc`、`Wasm`、`Dsl`、`OpenidConnect` | `fail-open`、`fail-close` |
| `ForwardAuth`、`LdapAuth` | 仅 `fail-close` 是既定策略（鉴权类插件永不 fail-open） |
| 其余插件 | 无 |

插件类型到能力的映射实现为 `EdgionPlugin::dye_capabilities`（`edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs`）——上表须与其保持一致；若两者出现分歧，以代码为准。

## Value 的两种来源

`value` 有两种来源，共用同一套规则结构：

- **不设置** —— 使用插件自身记录的值：
  - 在 `success` 上，是已解析的选择结果（例如当前生效的 profile 名、解析出的 backendRef `[ns/]name`、解析出的目标 `domain:port`、解析出的区域名）——与各插件此前已有的 dye 语义保持一致；
  - 在失败结果上，是 `<plugin_type>:<behavior>:<cause>`——snake_case 的插件类型、结果名，以及插件自行编写的诊断性 cause，例如 `rate_limit_redis:fail-open:redis-unreachable`、`canary:reject:on-invalid`。
- **设置** —— 一个基于共享 `${type:name}` 引擎的运维自定义模板字符串（参见[统一点注册表条目 11](../../../../../../skills/01-architecture/00-common/05-unification-points.md)），其中纯字面量（如 `value: degraded`）是模板的退化形式。允许的变量类型是该引擎支持的**除 `secretRef` 之外**的全部类型——dye 的 value 会直接渲染进客户端可见的请求头，因此绝不能让 secret 类变量出现在这里，否则等同于通过配置把 secret 外泄出去。客户端可控的变量（`header`/`query`/`cookie`）是允许的：它们只是回显了客户端本就知道的信息，并不构成运维未预期的信息泄露。某次请求中若引用的变量不存在，会渲染为空字符串（模板引擎一贯的 `ctx_set` 约定），因此模板化的 value 在客户端未提供对应值时会退化成其字面骨架，而不会报错。

## 稳定性契约（失败 value）

对于不设置 value 时的失败场景，`<plugin_type>:<behavior>` 是稳定契约：`plugin_type` 是固定的 snake_case，`behavior` 是上面那个封闭枚举去掉 `success` 后的取值。下游工具可以匹配这个前缀。末尾的 `cause` 段是**尽力而为的诊断信息**，可能在 Edgion 版本之间发生变化而不另行通知——不要把它当作 API 去解析。成功场景的 value 保持原有契约（裸的已解析值，无前缀）。

如果你需要比完整 `cause` 更少的信息暴露，可以用静态 `value` 覆盖（例如 `value: degraded`）——它会完全丢弃 behavior/cause 细节，转而渲染你指定的字面量。

## 防伪造剥离（Anti-spoof strip）

在插件运行之前——早于它的 `enable: false` kill-switch、任何协议限定，以及它自身的条目级 `conditions`——运行时会从入站请求中移除该条目所配置的**请求方向**规则名对应的请求头。这只针对客户端提供的值，是"先剥离、命中后再渲染"的固定顺序，而不是一个开关：

- 剥离**每个请求头名在每个请求中只执行一次**，在流水线中第一个声明该名称的条目处执行——由更早的条目自身的请求方向 dye 规则渲染出来的值，是网关自己写入的，不会被后面共享同一个头名的条目（或后面的运行时层级——route → backend → gateway → global 依次执行）再次剥离；
- 即使插件被 kill-switch（`enable: false`）关闭，或被它自身的条目级 `conditions` 跳过，剥离依然会执行——conditions 只决定规则是否*渲染*，从不决定剥离是否执行。一个被禁用或被 conditions 跳过的插件，其配置的 dye 名称仍然会从客户端输入中被移除。

这意味着，无论匹配的插件在某次请求中走了哪条路径，客户端都永远无法伪造路由已配置用于染色的名称下的值。

## 请求头名校验

每个配置的 `name` 都会在准入阶段被校验（Controller 摄入与 Gateway 编译共用同一份实现）：

- 必须通过标准 HTTP header-token 语法检查；
- 不能是以下与消息帧相关的禁用名称之一：`Host`、`Content-Length`、`Transfer-Encoding`、`Connection`、`TE`、`Upgrade`、`Trailer`；
- （仅请求方向）不能与该插件自身的 header 型 key 来源同名（大小写不敏感），例如 `DynamicInternalUpstream` 的 dye 名与它自己 `from: {type: header, name: ...}` 冲突——否则防伪造剥离会在插件读取自己的路由输入之前就把它删掉；
- （响应方向）在同一个 `EdgionPlugins` 资源内，失败规则的 `name` 不能与任意成功规则的 `name` 同名（大小写不敏感）——否则排在失败 Append 之后的成功 Set 会静默清空已追加的失败值。这个校验只覆盖单个资源内部；如果你通过多个 `ExtensionRef` 从多个 `EdgionPlugins` 资源组合插件，请自行为成功和失败染色使用不同的名称，因为跨资源组合无法做静态校验。

## `conditions`

每条规则都可以携带一个可选的 `conditions` 块，复用与条目级 `conditions` 相同、显式包含 ConditionSet 的 `PluginConditions` `run`/`skip` 结构。变体、`negate` 与 `values` 见 [插件条件](./conditions.md)。与剥离（无条件执行）不同，`conditions` 决定的是一个*已经匹配*的结果是否真正渲染其请求头——这也是面向公网路由的主要安全缓解手段（见下文）。

## 安全指引

染色值——尤其是失败染色值——会向路由上的每个客户端暴露内部状态：

- `rate_limit_redis:fail-open:redis-unreachable` 会实时告诉客户端限流当前未生效，探测型客户端可以借此把滥用行为的时机对准 fail-open 窗口；
- `ldap-unreachable` 之类的 cause 会暴露具体哪个内部依赖出现故障（具备侦察价值）。

请把 `dye`——尤其是失败结果染色——当作面向内部、金丝雀或可信层流量的可选（opt-in）观测手段，而不是面向公网路由默认开启的功能。对于面向公网的路由，可以：

- 在你的外层（CDN/入口网关）剥离该请求头，使其不会到达公网客户端；或
- （推荐）为规则配置 `conditions`，例如校验一个调试 token 请求头，使该结果只在你自己发起的诊断请求上渲染。

如果你既想要可观测性又不想暴露过多信息，可以使用静态 `value` 覆盖——它会丢弃 `cause`（乃至 `behavior`）细节，渲染一个固定字面量，而不是插件记录的诊断字符串。

## 相关文档

- [Canary](./canary.md)、[Dynamic Upstream](./dynamic-upstream.md)、[Direct Endpoint](./direct-endpoint.md)、[RegionRoute](./region-route.md)——各插件特有的 value 来源与示例。
- [过滤器总览](../overview.md)
- 设计文档：[`2026-07-20-entry-level-dye-suite-design.md`](../../../../../../docs/superpowers/specs/2026-07-20-entry-level-dye-suite-design.md)（完整机制、校验与运行时渲染细节）。
