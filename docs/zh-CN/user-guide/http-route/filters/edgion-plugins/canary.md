# Canary 插件

> **🔌 Edgion 扩展**
>
> Canary 是 `EdgionPlugins` CRD 提供的全量切换（full-cutover）流量插件，不属于标准 Gateway API。

## 概述

`Canary` 会把路由的**全部流量（100%）**一次性发送给某一个命名的后端——即"当前生效 profile"——而不是按权重拆分。它是 `backendRefs[].weight` 百分比拆分的全量切换 / 蓝绿部署对应版本：weight 是逐步转移一部分流量,而 Canary 是原子化地将**全部**流量从一个发布版本切到另一个。

当前生效的 profile 可以来自两处：

- **基础配置**（`EdgionPlugins` 资源中的 `activeProfile`）——由 git 管理，切换需要重新部署。
- **运行时覆盖**（`activeProfileRef`，指向一个 `Selector` 类型的 `EdgionConfigData`）——无需改动 `EdgionPlugins` 资源即可热切换；更新引用的 `EdgionConfigData` 会在标准 conf_sync 热重载窗口内，让所有引用它的网关一起切换生效 profile。

每个命名 profile 都指向一个 `backendRef`，该 `backendRef` **必须已经列在路由的 `backendRefs` 中**——Canary 只在路由已有的后端之间做选择，不能跳转到任意 Service。

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: canary-switch
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: Canary
      dye:
        request:
          - name: X-Canary-Active
      config:
        enable: true
        activeProfile: stable
        profiles:
          stable:
            name: canary-app-v1
          canary:
            name: canary-app-v2
        onInvalid: reject
```

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: canary-test
  namespace: default
spec:
  parentRefs:
    - name: edgion-plugins-gateway
      namespace: edgion-test
      sectionName: http
  hostnames:
    - "canary-test.example.com"
  rules:
    - backendRefs:
        - name: canary-app-v1
          port: 30001
        - name: canary-app-v2
          port: 30002
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: canary-switch
```

未配置 `activeProfileRef` 时，所有请求都被固定路由到 `stable` profile（`canary-app-v1`）——即路由 100% 的流量，无论路由上 `backendRefs[].weight` 如何设置（只要 Canary 启用且成功解析出有效 profile，就会覆盖加权选择）。

## 配置参考

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable` | Boolean | **必填** | Kill-switch。为 `false` 时插件是纯粹的直通（no-op）；路由自身基于权重的 `backendRefs` 选择不受影响，原样生效。 |
| `activeProfile` | String | **必填** | 基础/默认 profile 名，必须存在于 `profiles` 中。未配置 `activeProfileRef` 时使用它；selector 解析出现任何错误时也回落到它（fail-to-base）。 |
| `profiles` | Map\<String, DynUpstreamTarget\> | **必填** | 命名 profile 集合：profile 名 -> 目标 backendRef。由 git 管理，始终可安全回落。不能为空。 |
| `profiles[].name` | String | **必填** | 目标 `backendRef` 名称（必须与路由中的某个 `backendRef` 匹配）。 |
| `profiles[].namespace` | String | 无 | 目标 `backendRef` 的可选命名空间。不设置时仅按名称匹配；设置时名称和命名空间都需匹配。 |
| `activeProfileRef` | EdgionConfigDataRef | 无 | 可选引用，指向一个 `Selector` 类型的 `EdgionConfigData`，在运行时提供当前生效的 profile 名。 |
| `onInvalid` | Enum | `Reject` | 解析出的 profile 对应的 `backendRef` 不在路由的 `backendRefs` 中时（profile 过期/配置错误）的行为。允许值：`Reject`（403）/ `Fallback`（回落到路由自身基于权重的 `backendRefs` 选择）。 |

`dye` 是**条目级（entry-level）**字段——与 `type`/`config`/`enable` 同级，不属于 `config` 的成员：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `dye` | Object | 无 | 按结果分类的流量染色请求头/响应头（`on: [success, fallback, reject]`）。`request`/`response` 为 `{name, on, value, conditions}` 条目列表；不设置 `value` 时，`success` 上携带已解析的 profile 名。完整参考见 [染色（Dye）](./dye.md)，Canary 特有的部分见下方[染色请求头](#染色请求头)。 |

### EdgionConfigDataRef（用于 `activeProfileRef`）

| 参数 | 类型 | 说明 |
|------|------|------|
| `name` | String | `EdgionConfigData` 资源名（`type: Selector`）。 |
| `namespace` | String | 命名空间。默认为 `EdgionPlugins` 所在命名空间。 |

准入校验（违反则 `Accepted=False`）：`profiles` 不能为空，且 `activeProfile` 必须命中 `profiles` 中的某个 key。

## 使用场景

### 仅基础配置：静态全量切换

不配置 `activeProfileRef` 时，Canary 就是路由加权选择的一个固定覆盖——所有请求都走 `activeProfile`。要切换生效的后端，需要修改并重新应用 `EdgionPlugins` 资源（即重新部署）：

```yaml
requestPlugins:
  - type: Canary
    config:
      enable: true
      activeProfile: stable
      profiles:
        stable:
          name: app-v1
        canary:
          name: app-v2
```

适用于切换决策属于 GitOps/CI 流程一部分，而非运维人员的实时操作的场景。

### 热切换：运行时可切换的全量切换

添加 `activeProfileRef`，指向一个 `Selector` 类型的 `EdgionConfigData`。翻转该 selector 的 `active` 字段即可将 100% 流量切到指定 profile，无需重新部署 `EdgionPlugins` 资源：

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: canary-selector
  namespace: default
spec:
  data:
    type: Selector
    config:
      active: stable   # 改为 "canary" 即可将 100% 流量切到 canary profile
```

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: canary-switch
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: Canary
      dye:
        request:
          - name: X-Canary-Active
      config:
        enable: true
        activeProfile: stable
        profiles:
          stable:
            name: canary-app-v1
          canary:
            name: canary-app-v2
        activeProfileRef:
          name: canary-selector
          namespace: default
        onInvalid: reject
```

把 `canary-selector.spec.data.config.active` 从 `stable` 改为 `canary`，会热切换所有引用它的网关——不需要修改 `EdgionPlugins`，不需要修改路由，也不需要重启。

### `onInvalid` 的选择：`reject` 与 `fallback`

如果解析出的 profile 对应的 `backendRef` 不在路由的 `backendRefs` 中（例如某个 profile 配置的名称已不再匹配路由中的任何 `backendRef`），`onInvalid` 决定接下来的行为：

```yaml
requestPlugins:
  - type: Canary
    config:
      enable: true
      activeProfile: stable
      profiles:
        stable:
          name: app-v1
        canary:
          name: app-v2   # 如果 app-v2 之后从路由的 backendRefs 中移除...
      onInvalid: reject  # ...解析到 "canary" 的请求会收到 403
```

- `reject`（默认）：返回 `403 Forbidden`，body 为 `"Target backend not allowed"`。失败关闭（fail closed）——适用于过期 profile 指向错误后端属于安全隐患的场景（例如可能因此把流量发到已下线或非预期的 Service）。
- `fallback`：回落到路由自身基于权重的 `backendRefs` 选择，效果等同于该请求上 Canary 未启用。失败开放（fail open）——适用于宁可走正常负载均衡也不希望直接拒绝请求的场景。

### 染色请求头

完整参考见 [染色（Dye）](./dye.md)：`on` 结果词表、两种 `value` 来源、模板引擎、防伪造剥离（即使插件被 `enable: false` 关闭或被 conditions 跳过也会执行）、请求头名校验，以及安全指引。本节只覆盖 Canary 特有的部分。

`dye` 默认关闭：不配置时，Canary 不设置任何请求头，也不做任何剥离。配置后，会附加运维自定义的 HTTP 头，方便后端（或观测系统）识别本次请求由哪个 profile 处理，或走了哪条 fallback/reject 路径：

```yaml
requestPlugins:
  - type: Canary
    dye:
      request:
        - name: X-Canary-Active      # 不设置 value => 已解析的 profile 名（默认 on: [success]）
      response:
        - name: X-Served-By
          value: edge-cluster-1      # 显式静态值
        - name: X-Edgion-Outcome
          on: [fallback, reject]     # 不设置 value => canary:fallback:on-invalid / canary:reject:on-invalid
    config:
      enable: true
      activeProfile: stable
      profiles:
        stable:
          name: app-v1
        canary:
          name: app-v2
```

```bash
curl -H "Host: canary-test.example.com" http://<gateway>/  # 上游会看到 X-Canary-Active: stable
```

- `success` 上不设置 `value` 时携带**已解析的 profile 名**（如 `stable`、`canary`）。
- Canary 支持 `success`、`fallback`、`reject` 三种染色能力——`fallback`/`reject` 规则会给 `onInvalid: fallback` / `onInvalid: reject` 路径染色（这是该插件仅有的失败类分支）；`enable: false` 和无路由都不是"结果"，因此不会染色。

## 行为细节

- **Kill-switch（`enable: false`）**：插件完全直通（no-op）；路由自身基于权重的 `backendRefs` 选择原样生效。访问日志记录 `disabled:pass`。
- **Selector 解析与 fail-to-base**：配置了 `activeProfileRef` 时，插件在**每个请求**上都会从引用的 `Selector` 类型 `EdgionConfigData` 解析当前生效的 profile 名（以支持热重载）。每个 fail-to-base 分支都回落到 `activeProfile`,并且发出**互不相同**的 `selector miss: <reason>` 日志标签,方便运维在排查"我这一下金丝雀切了没"时,直接在 access log 里 grep 对应失败原因:

  | 原因                             | Access log 标签                       | 触发场景                                                                                              |
  |----------------------------------|--------------------------------------|-------------------------------------------------------------------------------------------------------|
  | 引用的对象不存在                  | `selector miss: absent`              | `activeProfileRef` 的 name/namespace 在 EdgionConfigData 存储里查不到。                                |
  | Envelope kill-switch 关闭         | `selector miss: disabled`            | 被引用的 `EdgionConfigData` 设置了 `spec.enable: false`。                                              |
  | 资源类型不对                      | `selector miss: type_mismatch`       | 被引用的 `EdgionConfigData` 类型不是 `Selector`（例如 `KeyList`、`IpList`）。                          |
  | Profile 名字未知                  | `selector miss: unknown_profile`     | `Selector.active` 指向的名字在本插件的 `profiles` 里不存在。                                            |

  被拒绝的跨命名空间引用在插件构造时就已处理：被 controller 标记为 `ref_denied` 的 `activeProfileRef`（没有匹配的 `ReferenceGrant`）会被视同"未配置 selector",插件根本不会去存储里查它,也不会打任何 `selector miss` 标签。被拒绝的引用永远回落到基础 `activeProfile`，不会被攻击者可控的值影响。

  有一种情形是刻意的直通而非失败：如果 `Selector` 的 `active` 字段未设置（`null`），插件同样会使用基础 `activeProfile`——但这不会被记录为 miss，因为未设置 `active` 是一种有效的"使用基础配置"配置，而不是错误。

  > 按项目约定,这些信号只走 access log 而不做 Prometheus 计数器——需要"按原因分类的金丝雀切换失败率"仪表盘时,在查询期由 Elasticsearch / ClickHouse / Loki 聚合出来即可。
- **backendRef 归属校验**：解析出 profile 名（无论来自基础配置还是 selector）之后，插件会校验该 profile 的目标 `backendRef`（`name` + 可选 `namespace`）确实出现在路由当前的 `backendRefs` 中。这个校验在**每个请求**上都针对路由的当前状态执行，因此从路由中移除某个后端会使指向它的任何 profile 失效——由 `onInvalid` 决定后续行为。
- **`onInvalid` 行为**：见上方"使用场景"一节。
- **`dye`**：默认关闭；配置后只在成功完成上游跳转时才会应用配置的请求/响应头（插件被禁用、无路由、或被拒绝时不会应用）。详见上方[染色请求头](#染色请求头)。
- **无路由 / 无 backendRefs**：如果请求没有路由单元，或路由规则没有 `backendRefs`，插件直接放行（`GoodNext`），不做任何动作——实践中不应出现此情况，因为 `Canary` 只有挂在带后端的路由上才有意义。

## 常见问题

### Q1：Canary 与 `backendRefs[].weight` 有何区别？

**A**：`weight` 对每个请求做**百分比拆分**（如 90/10），要改变比例需要修改路由。`Canary` 始终把 **100%** 流量发给某一个 profile，并可通过 `activeProfileRef` 在运行时切换，无需改动路由或重新部署。渐进式放量百分比用 `weight`；即时的、可远程切换的全量切换（蓝绿风格）用 `Canary`。参见[后端权重](../../backends/weight.md)。

### Q2：Canary 与 `DynamicInternalUpstream` 有何区别？

**A**：`DynamicInternalUpstream` 是**按请求**选择目标，由该请求自身的元数据（header/query/cookie 值）驱动——不同请求可以同时被路由到不同后端（例如某个金丝雀 cookie 把一个用户固定住,其余用户走正常加权选择）。`Canary` 是**对整个路由全局**选择目标，由配置/selector 状态驱动——在 selector 改变之前,所有请求都走同一个 profile。参见 [Dynamic Upstream](./dynamic-upstream.md)。

### Q3：如果我彻底移除 `activeProfileRef` 会怎样？

**A**：插件始终使用基础 `activeProfile`。这与 selector 解析结果为"使用基础配置"的最终状态完全相同——"未配置 `activeProfileRef`"与"配置了但当前解析失败/缺失/被拒绝"之间没有行为差异。

### Q4：可以给每个 profile 附加不同的业务自定义请求头（而不只是携带已解析 profile 名的固定头）吗？

**A**：`dye` 支持任意运维自定义的请求头名和方向，值可以是基于共享 `${type:name}` 引擎的模板（不允许 `secretRef`），也可以不设置（携带插件记录的值）——这已经覆盖了常见业务场景（`X-Release-Track: <当前生效 profile>`），模板形式也覆盖了大多数自定义业务头场景。按 profile 配置不同的 `{name, value}` 覆盖列表目前仍是刻意的非目标（non-goal）：常见业务场景已被"不设置 value 即用已解析值"以及模板覆盖，若未来出现具体需求可以再补充。完整参考见 [染色（Dye）](./dye.md)，完整设计参见设计文档 [`2026-07-20-entry-level-dye-suite-design.md`](../../../../../../docs/superpowers/specs/2026-07-20-entry-level-dye-suite-design.md)。

## 相关文档

- [Dynamic Upstream](./dynamic-upstream.md) —— 由请求元数据驱动的按请求上游选择
- [请求访问策略](./request-access-policy.md) —— 另一个支持 Selector profile 的插件
- [后端权重](../../backends/weight.md) —— 基于百分比的流量拆分
- [过滤器总览](../overview.md)
