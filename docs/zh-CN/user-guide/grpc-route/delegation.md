# GRPCRoute 委派（多租户）

GRPCRoute 委派允许平台团队把某个 **gRPC 服务**的路由所有权交给业务团队的
namespace，而无需给对方任何 Gateway 或根路由的写权限。父 GRPCRoute 的规则把
`backendRef` 指向子 GRPCRoute（而不是 Service）；controller 在编译期把委派树
摊平成扁平路由表，数据面完全感知不到嵌套。

它与 [HTTPRoute 委派](../http-route/delegation.md) 一一对应，只是把被委派的
"范围"重新定义为 gRPC 语义：不再是路径前缀，而是一个 **gRPC 服务名**（例如
`billing.v1.PaymentService`）。方法级委派与"委派全部"（不带 service 的委派
match）在 v1 中不支持。

## 为什么需要

在多租户集群里，只有平台 namespace 的路由能挂到共享 Gateway（`allowedRoutes`
默认 `Same`）。没有委派时，业务方的每次路由变更都要经过平台团队的根路由。有了
委派，平台团队为每个租户服务写一条委派规则，租户在自己的 namespace 里自由管理
子路由——安全边界就是 RBAC。

## 快速开始

父（根）路由，由平台团队维护在 Gateway 所在 namespace：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: root
  namespace: infra
spec:
  parentRefs:
    - name: shared-gateway
  hostnames: ["grpc.example.com"]
  rules:
    - matches:
        - method:
            type: Exact                # Exact 或省略；method 必须缺省
            service: billing.v1.PaymentService
      filters:                         # 对所有委派流量强制生效
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set: [{ name: x-tenant, value: billing }]
      backendRefs:
        - group: gateway.networking.k8s.io
          kind: GRPCRoute
          name: "*"                    # 该 namespace 下所有 GRPCRoute
          namespace: billing
```

子路由，由 billing 团队维护在自己的 namespace：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: billing-routes
  namespace: billing
spec:
  # 不需要 parentRefs：只能通过委派进入流量路径。
  rules:
    - matches:
        - method:
            type: Exact
            method: Charge              # service 继承自委派范围
      backendRefs:
        - name: payment-svc
          port: 50051
```

## 语义

### 委派规则

规则的唯一 `backendRef` 为 `group: gateway.networking.k8s.io, kind: GRPCRoute`
时即为委派规则。约束：

- 必须恰好一个 backendRef（不能与 Service ref 混用）。
- 不能设置 `port` 和 per-backendRef `filters`；`weight` 被忽略。
- `matches` 必须存在且非空。**与 HTTPRoute 委派不同**：无 match 的委派规则在
  这里是非法的——HTTPRoute 委派把无 match 的委派规则视为委派 `"/"`，但"委派
  全部"在 gRPC v1 中不支持，因此这个默认行为**故意不镜像**。
- 每个 match 都必须带一个 `method` matcher：`type: Exact`（或省略，按 Gateway
  API 语义等价于 Exact）、非空 `service`、且不能设置 `method`。没有 `method`
  matcher（仅 headers）、`type: RegularExpression`，或设置了 `method.method`
  的 match，都会使整条委派规则非法。
- `name: "*"` 表示委派给目标 namespace 下的所有 GRPCRoute（不含父路由自身）；
  `namespace` 缺省为父路由所在 namespace。
- 一条规则若有多个 match，会各自独立委派其 service（各自带自己的 `headers`
  约束）给同一批子路由。

不合法的委派规则会被丢弃并通过路由的 `PartiallyInvalid` 条件上报；指名的子
路由不存在时通过 `ResolvedRefs=False / BackendNotFound` 上报。

### 子路由

- 子 match 的 `method.service` **非空**时，必须与该委派规则的某个被委派
  service 字符串相等；不相等则该 match 越界并被丢弃（规则本身如果还有其他
  合法 match 则继续存活）。
- 子 match 的 `method.service` **缺失或为空**——完全没有 `method` matcher、
  空的 `method: {}`，或只写了 method 的 matcher——会继承被委派的 service：
  摊平后变成 `method: {type: Exact, service: S}`（保留子路由自己写的
  `method.method`，如果有）。若委派了多个 service，这类 match 会按每个
  scope 各展开一次。
- **无 match 的子规则**（`matches` 缺失或为空）会摊平成**每个委派 scope 一个
  继承 match**：`matches: [{method: {type: Exact, service: S}}]`（附带合并
  后的 header 约束）——绝不会原样克隆。这是安全关键点：gRPC 网关会把无
  match 的规则编译成 catch-all 单元，原样克隆会命中父路由 hostname 下的
  **所有**流量。
- 子 match 里出现 `type: RegularExpression` 会被拒绝——整条子规则非法（丢弃
  并上报）。v1 直接拒绝正则 match，而不是去证明某个正则 service 无法逃出
  委派范围。
- 没有任何合法 match 的规则会被丢弃并上报。
- 子路由**不能**设置 `spec.hostnames`（hostname 由根路由 / listener 决定）；
  违规的子路由整体跳过。
- 子路由可以继续向下委派（多级）。循环引用会被拒绝，链条深度上限为 8。
- 子路由可通过 `parentRefs` 中 `kind: GRPCRoute` 的条目**反向锁定**允许哪些
  父路由委派给它。不写则接受任何父的委派（委派本身由父授权——信任根是父
  namespace 的写权限）。

### 策略继承（父 filter 强制）

- 委派规则上的标准 filter（`RequestHeaderModifier`、`ResponseHeaderModifier`）
  自动继承到每条摊平后的子规则上，且**不可覆盖**：子规则声明了与继承类型相同
  的标准 filter 时整条规则被丢弃（经 `PartiallyInvalid` 上报）。GRPCRoute
  没有 `RequestRedirect` / `URLRewrite`，因此不存在这两类冲突。
- `ExtensionRef`（EdgionPlugins）例外，采用串联：父插件链先执行，然后是子
  插件链。
- 委派 match 的 `headers` 约束会（大小写不敏感地）合并进每条摊平后的子
  match；出现冲突（同名 header 但值或匹配类型不同）会丢弃该子规则。约束会
  沿多级委派链累积。
- `timeouts` / `retry` / `sessionPersistence`：子规则自己写了用子的，否则
  继承最近一级祖先的值。

### 后端与 ReferenceGrant

子路由的 `backendRefs` 保持子路由本地语义：不写 `namespace` 表示**子路由**
所在 namespace；跨 namespace 的 Service 引用按"子路由为引用方"检查
ReferenceGrant。

### 优先级与冲突

摊平把子路由的规则搬到父路由上，因此在数据面上，委派规则和父路由自己的普通
规则**同等匹配**：它们都乘着父路由的 `resolved_hostnames`，gRPC 匹配引擎的
优先级分桶（exact service + exact method → exact service + regex method →
regex service + exact method → regex service + regex method →
仅 exact service → 仅 regex service → catch-all）只是多了一些条目。同一个桶
内，平局按 `header_count > creation_ts > route_key > rule_index > match_index`
决胜。

以下两点仅在多条路由命中**完全相同的 service/method 且 specificity 相同**
时才有影响——这本身是误配，但结果是确定的：

- **委派子规则继承父路由的身份做 tie-break。** 摊平后的子规则用的是
  **父路由**的 `creation_ts_ms` 和 `route_key`，而不是子路由自己的。所以一
  个新建的子路由，若被一个老父路由委派，会在与同 hostname 上另一条无关路由
  撞车时继承父的（更老、更高）优先级。
- **父自己的规则 vs 委派子规则命中相同 service/method：** 由于两者的
  `creation_ts`/`route_key` 都是父路由的（平局），胜者由 `rule_index` 决
  定——也就是它们在父路由摊平后 `resolved_rules` 里的位置，这个位置保持
  `spec.rules` 的书写顺序（委派规则的展开结果落在委派规则原来的位置上）。
  把父自己的规则写在委派规则**之前**即可保证平台规则赢；写在之后则子规则
  赢。同一通配委派下的两个子路由之间，同样由 `rule_index` 决定，而通配展开
  按 `"namespace/name"` 升序枚举子路由——因此字母序更靠前的子路由赢得平局。

以上都不会让子路由逃出被委派的 service；只决定两条命中相同 service/method
的规则谁被选中。

## 状态

- **父路由**：指名子路由缺失 → `ResolvedRefs=False/BackendNotFound`；每条被
  丢弃的规则（service 越界、正则 match、filter 冲突、循环、超深、委派规则不
  合法）→ `PartiallyInvalid=True` 并列出原因。
- **子路由**：每个 `kind: GRPCRoute` 的 `parentRefs` 条目会得到一个
  `Accepted` 条件，报告该父当前是否委派给它（`DelegationAccepted` /
  `NoMatchingParent`）。

## 兼容性

委派仅在 `backendRef {kind: GRPCRoute}` 时激活，而这种写法此前是非法配置——
存量路由行为完全不变。`allowedRoutes` 语义不受影响（默认仍是 `Same`）；子
路由从不挂 listener，listener 策略对其不适用。运行时未命中的 gRPC 请求仍然
沿用现有网关行为（`UNIMPLEMENTED`，gRPC 状态码 12），而不是 HTTP 404。

## 可观测性

摊平后的规则带有生成的规则 `name`（`<子ns>.<子名>.r<规则下标>`，子规则自己
命名则保留原名），日志与 status 中引用规则名即可定位所属子路由。
