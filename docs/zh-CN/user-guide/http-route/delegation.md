# 路由委派（多租户）

路由委派允许平台团队把某个路径前缀的路由所有权交给业务团队的 namespace，而无需
给对方任何 Gateway 或根路由的写权限。父 HTTPRoute 的规则把 `backendRef` 指向子
HTTPRoute（而不是 Service），每个子路由也在自己的 `parentRefs` 中声明这个父路由。
Gateway 按层级逐级匹配请求：先匹配根路由，再匹配命中的根规则所委派的子路由，依此
类推，直到最终选出后端的那条规则。

## 为什么需要

在多租户集群里，只有平台 namespace 的路由能挂到共享 Gateway（`allowedRoutes`
默认 `Same`）。没有委派时，业务方的每次路由变更都要经过平台团队的根路由。有了
委派，平台团队为每个租户前缀写一条委派规则，租户在自己的 namespace 里自由管理
子路由。委派是一条所有权边界：子路由只能收窄父路由交给它的流量，无法影响该范围
之外或更上层的匹配。

## 快速开始

父（根）路由，由平台团队维护在 Gateway 所在 namespace：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: delegation-parent
  namespace: edgion-test
spec:
  parentRefs:
    - name: delegation-gateway
  hostnames: ["delegation.example.com"]
  rules:
    - matches:
        - path: { type: PathPrefix, value: /api/v1/billing }
      filters:                       # 对所有委派流量强制生效
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set: [{ name: x-parent-filter, value: "on" }]
      backendRefs:
        - group: gateway.networking.k8s.io
          kind: HTTPRoute
          name: "*"                  # edgion-deleg 中所有声明了本父路由的 HTTPRoute
          namespace: edgion-deleg
    - matches:
        - path: { type: PathPrefix, value: /api }
      backendRefs:                   # 平台自己的 /api 兜底规则
        - name: test-http
          port: 30001
```

子路由，由 billing 团队维护在自己的 namespace：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: billing-child
  namespace: edgion-deleg
spec:
  parentRefs:                        # 必填：子路由必须声明父路由
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      name: delegation-parent
      namespace: edgion-test
  rules:
    - matches:
        - path: { type: PathPrefix, value: /api/v1/billing/pay }   # 绝对路径
      backendRefs:
        - name: child-svc
          port: 30001
```

`GET /api/v1/billing/pay` 先命中父路由的委派规则，再命中子路由的规则，最终带着
`x-parent-filter: on` 到达 `child-svc`。`edgion-tests/integration/conf/HTTPRoute/Delegation/`
下的 fixture 是本页所有场景的完整可用示例。

## 构建委派树

### parentRefs 形态

路由的 `parentRefs` 决定它的角色，两种角色互斥：

| 形态 | 角色 |
| --- | --- |
| 一个或多个 `kind: Gateway` 引用 | **根**路由，照常挂到 listener。 |
| 1–3 个 `group: gateway.networking.k8s.io, kind: HTTPRoute` 引用 | **子**路由，从不挂到 listener。 |
| 没有 `parentRefs` | 惰性路由，永远不可达。 |

混用 Gateway 引用与 HTTPRoute 引用、声明超过 3 个父路由、或引用其他路由类型（例如
`kind: GRPCRoute`），都会使整条路由无效：每个 `parentRefs` 条目报告
`Accepted=False / InvalidParentRefs`，路由不服务任何流量。子路由还必须让
`spec.hostnames` 保持为空，因为 hostname 归根路由所有；设置了 hostnames 的子路由是
惰性的，并报告 `Accepted=False / UnsupportedValue`。

### 委派规则

当规则唯一的 `backendRef` 为 `group: gateway.networking.k8s.io, kind: HTTPRoute`
时，该规则即为委派规则。约束：

- 恰好一个 backendRef，不能与 Service 引用混用。
- 不能设置 `port` 和 backendRef 级 `filters`；`weight` 被忽略。
- 所有 match 必须使用 `PathPrefix`。没有 matches 的规则、或没有 path 的 match，
  委派的是 `PathPrefix /`。委派规则上可以带 header、query、method 条件，它们在
  父路由这一层检查。因此只写 header、query 或 method 的 match，会把 `/` 下所有
  满足条件的路径都委派出去：请加上 `PathPrefix` 收窄委派范围。
- `name: "*"` 委派给目标 namespace 中所有在 `parentRefs` 里声明了本父路由的
  HTTPRoute。`namespace` 默认为父路由的 namespace。
- 自行选择上游的插件只能用在终结规则上。委派规则上的 ExtensionRef 若启用了
  `Canary`、`DirectEndpoint`、`DynamicInternalUpstream`、`DynamicExternalUpstream`、
  `RegionRoute`、`GraphqlOperationRouter`、`AllEndpointStatus` 或 `AiProxy`，该规则
  无效。

格式错误的委派规则会被丢弃，并通过父路由的 `PartiallyInvalid` 条件报告；同一路由的
其他规则继续服务。

### 双向确认

父路由到子路由的边只有在**双方都同意**时才存在：

1. 父路由的某条规则委派到该子路由（按名称，或通过子路由所在 namespace 的
   `name: "*"`），**并且**
2. 子路由在 `parentRefs` 中列出了这个父路由。

子路由的 `parentRefs` 条目就是子团队的同意，不涉及 ReferenceGrant。按名称委派、
子路由存在但没有列出该父路由时，父路由报告 `ResolvedRefs=False / RefNotPermitted`。
通配符只会选中声明了该父路由的子路由；匹配到零个子路由不算错误。

### 深度：最多 5 层

一条委派链最多 **5 层（含根路由）**：根路由加 4 条委派边。第 5 层的路由仍可以有
终结规则，但它的委派规则永远不会命中。本应位于第 6 层的子路由报告
`Accepted=False / DepthExceeded`。`chain-l1` … `chain-l6` fixture 演示了这一上限。

每个父路由的子路由数量没有上限。环不会被检测或报告，也不会造成死循环（见
[回溯](#回溯与-先进入者决定-限制)）。无法从挂到 Gateway 的根路由到达的环报告
`NotAttached`。

## 匹配

### 子路由路径是绝对路径；范围取交集

子路由写**绝对**路径。委派从不拼接或剥离父前缀。请求必须先匹配父规则的
`PathPrefix`，再匹配子规则自己的路径，所以实际范围是两者的交集：

- 位于父前缀之内的子路径（`/api/v1/billing` 下的 `/api/v1/billing/pay`）服务该
  子路径。
- 与父前缀不相交的子路径（`/api/v1/billing` 下的 `/evil`）永远不会经由该父路由
  命中。它不会被拒绝，也不会被报告。
- 没有 path、或 path 为 `PathPrefix /` 的子 match 覆盖整个委派范围。
- 子路由的 `RegularExpression` 等价于“父前缀 AND 子正则”。
- 一个子路由可以服务多个父路由，在每个父路由下服务与该父前缀的交集。在
  `multi-child` fixture 中，`/v1/billing/invoice` 经由 `billing-v1-parent` 服务，
  `/v2/billing/invoice` 经由 `billing-v2-parent` 服务。

整条链在任何过滤器或插件运行之前，针对原始请求完成匹配。父路由的 `URLRewrite`
或父插件添加的 header 不会改变子路由的匹配。

### 优先级按层比较

每一层只用普通的[匹配优先级](matches/README.md)对本层规则排序：路径特异度、
method、header 数量、query 参数数量、`creationTimestamp`、`namespace/name`、
规则/匹配序号。父路由的条件不计入子路由，子路由的条件也不会向上冒泡。

因此，更具体的子路径不会在根层级胜过另一条根路由。在 `precedence-*` fixture 中，
根路由 A 把 `/api` 委派给带 `Exact /api/users` 的子路由，根路由 B 在同一 hostname
上有 `PathPrefix /api/users`。`/api/users` 在根层级由 B 胜出。租户因此无法抢走
兄弟根路由的流量。

### 回溯与“先进入者决定”限制

委派规则不会结束匹配。自身条件匹配后，Gateway 进入被委派的子路由继续匹配：

- 如果某条子规则命中（直接命中或经由更深的委派），匹配链完成。
- 如果下层没有任何命中，该委派规则视为未命中，Gateway 会**回溯**：依次尝试同一层
  的下一个候选、下一个 hostname 分组、正则路由，最后返回 404。在
  `delegation-parent` fixture 中，`/api/v1/billing/unknown` 进入 billing 子路由、
  未命中，最终由父路由自己的 `/api` 规则服务。

每个被委派的子树**每个请求最多进入一次**。如果后续候选指向本请求已经尝试过的子树，
该候选直接跳过。这限制了单请求的工作量，也让环无害。

第一个进入子树的路径决定该子树在本请求中的结果。以下两种形态中，即使另一条路径
本可以命中，也可能返回 404：

- **不等长的菱形**：子路由可以经两条长度不同的路径到达，第一条路径到达它时所在的
  深度已不允许它的委派规则继续下探。
- **混合强制过滤器**：同一父路由的两条规则以不同的标准过滤器委派到同一目标。它们
  共享一个子树，因此由第一条规则的过滤器冲突决定尝试哪些子规则。

这是已记录的限制，不是 bug。生产环境的委派树应避免这些形态：每个父路由只用一条
规则委派同一目标；当子路由有多条到根的路径时，保持各条链长度一致。

## 继承

过滤器和插件在每个阶段（请求、上游请求、上游响应、响应、日志）都按链顺序
根 → … → 叶执行，同一层内保持编写顺序。响应阶段同样是父路由先执行。

- **标准过滤器总会执行。** 委派规则上的标准过滤器（`RequestHeaderModifier`、
  `ResponseHeaderModifier`、`RequestRedirect`、`URLRewrite`、`RequestMirror`、
  `CORS`、`ExternalAuth`）作用于经它委派的所有流量。若子规则（无论在规则本身
  还是在某个 `backendRefs[].filters` 上）声明了链上已存在类型的标准过滤器，该子规则
  在这个父路由下被跳过，子路由在该父路由的条目上报告 `PartiallyInvalid=True`。
  跳过和其他未命中一样会触发回溯，请求可能由其他规则（包括父路由自己的兜底规则）
  服务，到达不同的后端。同一子规则在没有冲突的其他父路由下仍然服务。
- **各层按顺序执行，后面的层看到前面的改动。** 每一层处理的是前面各层改过之后的
  请求，响应阶段同样按链顺序执行。Gateway 不阻止下层改动上层设置的内容：子路由的
  `ProxyRewrite` 可以重新设置父路由删掉的 header，子路由的 `ResponseRewrite` 可以
  改父路由设置的 header，子路由的 `RequestRedirect` 会用父路由 `URLRewrite` 改写后的
  路径和 `Host` 生成 `Location`。Gateway API 禁止在同一规则里同时使用 `URLRewrite` 和
  `RequestRedirect`，但委派不跨层检查这对组合。这些交互由子路由的负责方自行处理；
  匹配本身始终使用原始请求。
- **EdgionPlugins 先父后子。** 每一层的 `ExtensionRef` 在声明它的路由所在 namespace
  中解析 EdgionPlugins：父插件在父 namespace，子插件在子 namespace。每一层的插件和
  模板（`${matchedHttpRoute:...}`、labels、annotations）看到的是本层自己的路由和
  规则，因此子路由无法改变父路由所属插件的配置。
- **每个请求只有一个 WAF。** WAF 为整个请求只选一个 owner，绝不会执行两次。委派链上
  默认由声明了 WAF 的最深层胜出，因此子路由可以替换父路由的 WAF。父路由若必须保留
  自己的 WAF，在 WAF 配置中设置 `delegationPrecedence: parent`；此时设置了该项的最外层
  胜过它下面的所有层。同一层内仍按 `priority` 规则选择，路由级 WAF 仍优先于 Gateway
  级和全局 WAF。`delegationPrecedence` 只对路由引用的 WAF 生效，写在 Gateway 级或
  全局 WAF 上不起作用；平台若必须对委派流量保留自己的 WAF，应在根路由上挂一个
  `parent` 的 WAF。
- **`timeouts` 和 `retry` 取自设置了它们的最深层级**，按子字段分别回退：先用叶子
  自己的值，否则用最近祖先的值。`timeouts.request` 与 `timeouts.backendRequest`
  分别回退，`retry.attempts`、`retry.backoff`、`retry.codes` 也分别回退。
- **`sessionPersistence` 作为整块继承**：叶子设置了就用叶子的块，否则用最近祖先的
  块，子字段从不跨层混合。
- **后端只取叶子。** 子路由 `backendRef` 未写 `namespace` 时指子路由的 namespace，
  跨 namespace 的 Service 引用以子路由为引用方检查 ReferenceGrant。
- **独占型 owner 在整条链上保持独占。** 整条匹配链上最多只能有一个 AI owner、
  gRPC-JSON 转码器或 GraphQL 操作路由器，规则与普通路由相同。
- 路由注解 `edgion.io/forward-raw-path` 与 `edgion.io/retry-non-idempotent` 只从
  叶子路由读取。

## 状态

### 父路由（带委派规则）

| 条件 | 含义 |
| --- | --- |
| `PartiallyInvalid=True` | 自身某条委派规则格式错误或带有选择后端的插件。若已没有可服务的规则，挂载变为 `Accepted=False / UnsupportedValue`。 |
| `ResolvedRefs=False / BackendNotFound` | 按名称委派的子路由不存在。 |
| `ResolvedRefs=False / RefNotPermitted` | 按名称委派的子路由存在，但没有在 `parentRefs` 中列出本父路由；或者规则指向的是本路由自己（路由不能委派给自己）。 |

子路由内部的问题从不报告在父路由上。

### 子路由

子路由为每个 `kind: HTTPRoute` 的 parentRef 生成一个 `status.parents` 条目：

| 条件 | 含义 |
| --- | --- |
| `Accepted=True / DelegationAccepted` | 父路由存在、委派到本子路由、可从根路由到达，且位于第 4 层或更浅。根路由指写了 Gateway `parentRefs` 且自身校验通过的路由；这里不检查 listener 是否接受它。 |
| `Accepted=False / NoMatchingParent` | 父路由不存在，或未委派到本子路由。 |
| `Accepted=False / NotAttached` | 父路由无法从根路由到达（包括没有根的环），或者父路由或它的某个祖先自身校验失败、不服务委派流量。父路由的任何自身校验错误都会导致这种情况，包括单独一条格式错误的普通规则：修好父路由后子路由即恢复。 |
| `Accepted=False / DepthExceeded` | 父路由位于第 5 层，本子路由将位于第 6 层。 |
| `Accepted=False / InvalidParentRefs` | `parentRefs` 形态无效（所有条目）。 |
| `Accepted=False / UnsupportedValue` | 子路由设置了 `spec.hostnames`（所有条目）；或者子路由的所有规则都超出该父路由的范围或与其过滤器冲突，在该父路由下没有任何规则能服务（仅该条目）。 |
| `PartiallyInvalid=True`（同时 `Accepted=True`） | 部分子规则在该父路由下永远无法服务。`Rule(s) outside parent <Kind> '<namespace/name>' scope: …` 列出 `Exact`/`PathPrefix` 路径不落在该父路由委派给本子路由的任何前缀内的规则（正则和不带路径的 match 不检查）；`Conflicting Rule(s) with parent <Kind> '<namespace/name>': …` 列出与所有能到达它的委派规则的标准过滤器都冲突的规则。 |

只有当某条目为 `Accepted=True` 时，Gateway 才会把子路由装入该父路由下。深度按到根
路由的最短路径计算。经同一父路由的更长路径在请求时仍可能超过 5 层，此时只是不再
下探。

## 可观测性

- **访问日志**：`match_info` 是按根 → 叶排序的数组，每个命中的层级一个元素。每个
  元素带有本层自己的路由（`rsc.kind` / `rsc.ns` / `rsc.name`）、本层编写的
  `rule_id` 和 `match_id`，以及本层的 `sv` 同步版本。普通路由输出单元素数组。参见
  [访问日志字段说明](../../ops-guide/observability/access-log.md)。
- **指标**：`route_namespace` / `route_name` 标注的是叶子路由。
- `AccessLogExtern` 的 `from: routeLabel` / `routeAnnotation` 读取叶子路由。

## 限制

- 共享子树的“先进入者决定”，见[回溯](#回溯与-先进入者决定-限制)。
- 委派请求永远不会写入响应缓存，也不会从响应缓存返回。缓存键和凭据检查只覆盖叶子
  路由的插件，因此对委派流量直接关闭缓存，避免响应跨越父路由的鉴权边界被共享。
- 冲突按规则报告，且只在确定时报告。子规则只和部分能到达它的委派规则冲突时
  （例如与父路由的 `/a` 规则冲突、与 `/a/b` 规则不冲突），状态里不会列出；经冲突
  规则进入的请求仍会被 Gateway 跳过。与祖父及更上层的冲突在请求时生效，但不会
  报告在状态里。
- 一个请求最多进入 64 个委派子树（回溯过程合计），超出后其余委派候选都视为未命中。
  空子树（目标没有已接受的子路由）直接跳过，不计入次数。
- 路径参数（例如 `/t/:tid` 中的 `:tid`）只从叶子规则的模式中提取，父路由层的插件
  拿不到父路由模式里的参数。
- CORS 预检请求（`OPTIONS`）由某一层 EdgionPlugins 的 `Cors` 配置应答；多层都配置时，
  由最深层的配置应答。
- 子路由按规则保存的插件内存状态（例如本地计数器）会在 Gateway 进程中增加或删除
  Gateway 时重新开始。
- 父路由和子路由的变更相互独立地生效。短时间内，某个请求可能看到新的父路由与旧的
  子路由，或者相反。

## 兼容性

委派只由 `backendRef {kind: HTTPRoute}` 和路由类型的 `parentRefs` 触发。直接挂到
Gateway 的路由行为完全不变。`allowedRoutes` 语义不变（默认仍为 `Same`）；子路由从不
挂到 listener，因此 listener 策略不作用于它们。
