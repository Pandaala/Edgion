# GRPCRoute 委派（多租户）

GRPCRoute 委派允许平台团队把某个 **gRPC 服务**的路由所有权交给业务团队的
namespace，而无需给对方任何 Gateway 或根路由的写权限。父 GRPCRoute 的规则把
`backendRef` 指向子 GRPCRoute（而不是 Service），每个子路由也在自己的 `parentRefs`
中声明这个父路由。Gateway 按层级逐级匹配调用：先匹配根路由，再匹配命中的根规则所
委派的子路由，直到选出后端的那条规则。

它与 [HTTPRoute 委派](../http-route/delegation.md)完全一致：相同的 parentRefs 形态、
双向确认、5 层上限、按层优先级、回溯、继承、状态原因和访问日志形态。本页只说明
gRPC 的不同之处：委派规则交出的不是路径前缀，而是一个 **gRPC 服务名**（例如
`test.TestService`）。不支持方法级委派，也不支持不带服务的全量委派。

## 快速开始

父（根）路由，由平台团队维护在 Gateway 所在 namespace：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: delegation-parent
  namespace: edgion-test
spec:
  parentRefs:
    - name: grpc-delegation-gateway
      sectionName: grpc
  hostnames: ["grpc-delegation.example.com"]
  rules:
    - matches:
        - method:
            type: Exact                # Exact 或省略；不能设置 method
            service: test.TestService
      filters:                         # 对所有委派流量强制生效
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set: [{ name: x-parent-filter, value: "on" }]
      backendRefs:
        - group: gateway.networking.k8s.io
          kind: GRPCRoute
          name: "*"                    # 该 namespace 中所有声明了本父路由的 GRPCRoute
          namespace: edgion-grpc-deleg
```

子路由，由业务团队维护在自己的 namespace：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: child
  namespace: edgion-grpc-deleg
spec:
  parentRefs:                          # 必填：子路由必须声明父路由
    - group: gateway.networking.k8s.io
      kind: GRPCRoute
      name: delegation-parent
      namespace: edgion-test
  rules:
    - matches:
        - method:
            type: Exact
            service: test.TestService
            method: SayHello
      backendRefs:
        - name: child-grpc-svc
          port: 30021
```

`edgion-tests/integration/conf/GRPCRoute/Delegation/` 下的 fixture 是完整可用的示例。

## 构建委派树

### parentRefs 形态与确认

规则与 HTTPRoute 相同，只是使用 `kind: GRPCRoute`：

- 一个或多个 `kind: Gateway` 引用构成**根**路由；1–3 个
  `group: gateway.networking.k8s.io, kind: GRPCRoute` 引用构成**子**路由；没有
  `parentRefs` 的路由是惰性的。
- 混用 Gateway 与 GRPCRoute 引用、超过 3 个父路由、或引用其他路由类型（例如
  `kind: HTTPRoute`），会使整条路由无效（所有条目 `Accepted=False / InvalidParentRefs`）。
- 子路由不能设置 `spec.hostnames`（`Accepted=False / UnsupportedValue`）。
- 只有当父规则委派到子路由（按名称或通过 `name: "*"`）**并且**子路由在
  `parentRefs` 中列出该父路由时，边才存在。不涉及 ReferenceGrant。
- 最多 **5 层（含根路由）**。环不会被报告，也不会死循环。

### 委派规则

当规则唯一的 `backendRef` 为 `group: gateway.networking.k8s.io, kind: GRPCRoute`
时，该规则即为委派规则。约束：

- 恰好一个 backendRef，不能与 Service 引用混用。
- 不能设置 `port` 和 backendRef 级 `filters`；`weight` 被忽略。
- `matches` 必须存在且非空。**与 HTTPRoute 不同，没有 matches 的委派规则无效**，
  因为 gRPC 不支持全量委派。
- 每个 match 必须带 `method` 匹配器，`type: Exact`（或省略），`service` 非空且不
  设置 `method`。只有 header 的 match、`RegularExpression` 匹配器、或设置了
  `method.method` 的匹配器，都会使整条委派规则无效。服务之外的 header 条件是允许的，
  在父路由这一层检查。
- 一条规则有多个 match 时，每个服务都委派到同一目标。
- `name: "*"` 委派给目标 namespace 中所有声明了本父路由的 GRPCRoute。`namespace`
  默认为父路由的 namespace。
- 选择后端的插件（`Canary`、`DirectEndpoint`、`DynamicInternalUpstream`、
  `DynamicExternalUpstream`、`RegionRoute`、`GraphqlOperationRouter`、
  `AllEndpointStatus`、`AiProxy`）只能用在终结规则上；用在委派规则上会使该规则无效。

格式错误的委派规则会被丢弃，并通过父路由的 `PartiallyInvalid` 条件报告。

## 匹配

### 子路由 match 是绝对的；范围取交集

父路由这一层已经确定了服务，因此每个子 match 只需要在该服务内匹配：

- 带 `service` 的子 match 只在其等于请求服务时命中。不同的服务（`child` fixture 中
  的 `test.OtherService` 规则）永远不会经由该父路由命中，也不算错误。
- 不带 `service` 的子 match（没有 method 匹配器，或只有 method）匹配任意服务，
  取交集后就是被委派的那个服务。
- **没有 matches 的子规则**是被委派服务内的兜底规则（`matchless-child` fixture）。
  这是安全的：其他服务已在根层级被排除。
- 普通 GRPCRoute 的 method 校验同样作用于子规则：空的 `method: {}`、空的 Exact
  service 或 method、或无效名称，会使该子规则无效。
- 一个子路由可以服务多个父路由（`multi-child` fixture），在每个父路由下只带该父
  路由的强制过滤器。

### 优先级与回溯

每一层只用普通的 [GRPCRoute 优先级](matches/overview.md)对本层规则排序。更具体的
子 match 不会在根层级胜过另一条根路由。

如果委派规则下层没有任何命中，Gateway 会回溯到同一层的下一个候选。在
`grpc-backtrack-parent` fixture 中，`test.TestService/StreamNumbers` 进入只服务
`SayHello` 的 `backtrack-child`，未命中后由父路由自己的兜底规则服务。每个被委派的
子树每次调用最多进入一次，由第一个进入的路径决定其结果，参见
[HTTPRoute 的这一限制](../http-route/delegation.md)。没有任何路由命中的调用仍返回
`UNIMPLEMENTED`（gRPC 状态码 12）。

## 继承

与 [HTTPRoute](../http-route/delegation.md) 相同：

- 委派规则上的标准过滤器（`RequestHeaderModifier`、`ResponseHeaderModifier`）
  总会执行。在规则本身或某个 `backendRefs[].filters` 上带有同类型过滤器的子规则在该
  父路由下被跳过，并报告为 `PartiallyInvalid`。被跳过的规则视为未命中，调用可能回溯
  到其他规则和后端。
- 各层按根 → 叶顺序执行，每一层看到前面各层的改动。Gateway 不阻止子路由插件改动
  父路由设置的 header，这由子路由的负责方自行处理。
- EdgionPlugins 先父后子；每一层的 `ExtensionRef` 在本层路由的 namespace 中解析。
- 每个调用只有一个 WAF owner：默认最深层的 WAF 胜出，除非某个祖先层的 WAF 设置了
  `delegationPrecedence: parent`。
- `timeouts` 和 `retry` 按子字段取自设置了它的最深层级。`sessionPersistence` 作为
  整块继承。
- 后端只取叶子，namespace 默认值和 ReferenceGrant 检查都以子路由为准。
- 整条链上最多一个独占型 owner（AI owner、转码器）。
- `edgion.io/retry-non-idempotent` 只从叶子路由读取。

## 状态

状态原因与 [HTTPRoute](../http-route/delegation.md) 相同，parentRef 使用
`kind: GRPCRoute`：

- **父路由**：自身格式错误的委派规则报告 `PartiallyInvalid`；按名称委派的子路由不存在
  时报告 `ResolvedRefs=False / BackendNotFound`；子路由没有列出本父路由时报告
  `ResolvedRefs=False / RefNotPermitted`。
- **子路由**，每个 `kind: GRPCRoute` parentRef 一个条目：
  `Accepted=True / DelegationAccepted`，或 `Accepted=False` 加
  `NoMatchingParent`、`NotAttached`、`DepthExceeded`、`InvalidParentRefs`、
  `UnsupportedValue` 之一；`PartiallyInvalid=True` 列出在该父路由下永远无法服务的
  子规则：`Rule(s) outside parent <Kind> '<namespace/name>' scope: …` 对应方法 match
  指向了委派服务以外的服务，`Conflicting Rule(s) with parent <Kind> '<namespace/name>': …`
  对应与所有能到达它的委派规则的标准过滤器都冲突的规则。这些规则覆盖了子路由的全部
  规则时，该条目改为 `Accepted=False / UnsupportedValue`。父路由自身校验失败（任何错误，
  包括单独一条格式错误的规则）时，其子路由为 `NotAttached`；规则指向本路由自己时报告
  `RefNotPermitted`。

## 可观测性

访问日志 `match_info` 是按根 → 叶排序的数组，每个命中层级一个元素，数组形态与
HTTPRoute 相同；每个 GRPCRoute 元素还额外带有 `rule_name` 和 `matched`。指标标注的是
叶子路由。

## 限制

[HTTPRoute 的限制](../http-route/delegation.md)同样适用：共享子树“先进入者决定”、
每个调用最多进入 64 个委派子树、委派调用不使用响应缓存、Gateway 进程内 Gateway 集合变化时子路由按规则的插件状态
重新开始、父子路由变更相互独立地生效。
