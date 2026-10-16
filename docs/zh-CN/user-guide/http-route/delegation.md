# 路由委派（多租户）

路由委派允许平台团队把某个路径前缀的路由所有权交给业务团队的 namespace，而无需
给对方任何 Gateway 或根路由的写权限。父 HTTPRoute 的规则把 `backendRef` 指向子
HTTPRoute（而不是 Service）；controller 在编译期把委派树摊平成扁平路由表，数据面
完全感知不到嵌套。

## 为什么需要

在多租户集群里，只有平台 namespace 的路由能挂到共享 Gateway（`allowedRoutes`
默认 `Same`）。没有委派时，业务方的每次路由变更都要经过平台团队的根路由。有了
委派，平台团队为每个租户前缀写一条委派规则，租户在自己的 namespace 里自由管理
子路由——安全边界就是 RBAC。

## 快速开始

父（根）路由，由平台团队维护在 Gateway 所在 namespace：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: root
  namespace: infra
spec:
  parentRefs:
    - name: shared-gateway
  hostnames: ["api.example.com"]
  rules:
    - matches:
        - path: { type: PathPrefix, value: /api/v1/billing }
      filters:                       # 对所有委派流量强制生效
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set: [{ name: x-tenant, value: billing }]
      backendRefs:
        - group: gateway.networking.k8s.io
          kind: HTTPRoute
          name: "*"                  # 该 namespace 下所有 HTTPRoute
          namespace: billing
```

子路由，由 billing 团队维护在自己的 namespace：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: billing-routes
  namespace: billing
spec:
  # 不需要 parentRefs：只能通过委派进入流量路径。
  rules:
    - matches:
        - path: { type: PathPrefix, value: /api/v1/billing/pay }   # 绝对路径
      backendRefs:
        - name: payment-svc
          port: 8080
```

## 语义

### 委派规则

规则的唯一 `backendRef` 为 `group: gateway.networking.k8s.io, kind: HTTPRoute`
时即为委派规则。约束：

- 必须恰好一个 backendRef（不能与 Service ref 混用）。
- 不能设置 `port` 和 per-backendRef `filters`；`weight` 被忽略。
- 规则中的每个 match 都必须是 `PathPrefix`。
- `name: "*"` 表示委派给目标 namespace 下的所有 HTTPRoute；`namespace` 缺省为
  父路由所在 namespace。

不合法的委派规则会被丢弃并通过路由的 `PartiallyInvalid` 条件上报；指名的子路由
不存在时通过 `ResolvedRefs=False / BackendNotFound` 上报。

### 子路由

- 子路由路径是**绝对路径**，必须按段边界落在被委派前缀之内（`/api/v1/billing`
  接受 `/api/v1/billing/pay`，不接受 `/api/v1/billingX`）。越界 Exact/PathPrefix 规则被丢弃并在父
  路由 status 上报。
- 包含检查在**归一化**（点段、百分号编码）之后进行，因此 `/api/../admin` 无法逃出
  `/api` 委派前缀。
- `Exact` / `PathPrefix` 保留静态包含检查。`RegularExpression` 支持普通正则语法，
  包括交替与非锚定模式；请求必须同时满足子正则和所有祖先的 `PathPrefix`。
  父子检查使用同一条规范化后的完整路径，不自动拼接或剥离父前缀。
  例如父 `/api` 下的 `^/api/?secret$` 可以匹配 `/api/secret`，但不能匹配
  `/apisecret`。父检查失败会继续尝试其他候选；与父范围完全不相交的正则合法，
  但永远不会命中，也不会因此产生越界状态错误。
- 父条件使用普通路径匹配器，支持 `/:param` 和 `/::` 字面冒号转义。
  Exact/PathPrefix 静态包含检查仍然保守，不会推导 `/:tenant` 包含 `/acme`。
- 终端子 match 没有 path 或为默认 `PathPrefix /` 时继承被委派前缀。
  继续委派的中间规则仍须显式指定父范围内的前缀；非根父范围下省略中间路径或
  使用默认根路径仍不支持。
- 子路由**不能**设置 `spec.hostnames`（hostname 由根路由 / listener 决定）；违规
  的子路由整体跳过。
- 子路由可以继续向下委派（多级）。循环引用会被拒绝，链条深度上限为 8。
- 子路由可通过 `parentRefs` 中 `kind: HTTPRoute` 的条目**反向锁定**允许哪些父路
  由委派给它。不写则接受任何父的委派（委派本身由父授权——信任根是父 namespace
  的写权限）。

### 策略继承（父 filter 强制）

- 委派规则上的标准 filter（RequestHeaderModifier、CORS、URLRewrite 等）自动继承
  到每条摊平后的子规则上，且**不可覆盖**：子规则声明了与继承类型相同的标准
  filter 时整条规则被丢弃（经 `PartiallyInvalid` 上报）。
- `ExtensionRef`（EdgionPlugins）例外，采用串联：父插件链先执行，然后是子插件链。
- 每个 `ExtensionRef` 仍属于声明它的 HTTPRoute：父 filter 在父路由 namespace 中解析
  EdgionPlugins，子 filter 摊平后仍在子路由 namespace 中解析。
- 委派规则 match 里的 `headers` / `queryParams` / `method` 约束会合并进子 match；
  对末级正则 match，只跳过条件不兼容的父 OR 分支，保留兼容分支；如果任一子
  match 没有兼容的父分支，则丢弃该子规则。末级 Exact/PathPrefix match 遇到
  冲突时仍丢弃整个规则。中间委派会跳过不兼容的父子配对；每个中间 match 都须
  保留至少一个兼容的父范围，否则丢弃该中间规则。
- `timeouts` / `retry` / `sessionPersistence`：子规则自己写了用子的，否则继承最近
  一级祖先的值。

### 后端与 ReferenceGrant

子路由的 `backendRefs` 保持子路由本地语义：不写 `namespace` 表示**子路由**所在
namespace；跨 namespace 的 Service 引用按"子路由为引用方"检查 ReferenceGrant。

### 优先级与冲突

摊平把子路由的规则搬到父路由上，因此在数据面上，委派规则和父路由自己的普通规则
**同等匹配**。以下两点仅在多条路由命中**完全相同的路径且 match specificity 相同**
（method / header / query 数量一致）时才有影响——这本身是误配，但结果是确定的：

- **委派子规则继承父路由的身份做 tie-break。** Gateway API 对同 specificity 的平局
  按路由的 `creationTimestamp`（越老越优先）再按 `namespace/name` 决胜。摊平后的子
  规则用的是**父路由**的时间戳和 key，而不是子路由自己的。所以一个新建的子路由，
  若被一个老父路由委派，会在与同 hostname 上另一条无关路由撞车时继承父的（更老、
  更高）优先级。
- **父自己的规则 vs 委派子规则命中相同路径：** 胜者由它们在父 `spec.rules` 里的
  **书写顺序**决定。把父自己的规则写在委派规则**之前**即可保证平台规则赢；写在
  之后则子规则赢。同一通配委派下的两个子路由之间，按子路由 key 升序决胜。

以上都不会让子路由逃出被委派的前缀；只决定两条命中相同路径的规则谁被选中。

## 状态

- **父路由**：指名子路由缺失 → `ResolvedRefs=False/BackendNotFound`；每条被丢弃
  的规则（越界、filter 冲突、循环、超深、委派规则不合法）→ `PartiallyInvalid=True`
  并列出原因。
- **子路由**：每个 `kind: HTTPRoute` 的 `parentRefs` 条目会得到一个 `Accepted`
  条件，报告该父当前是否委派给它（`DelegationAccepted` / `NoMatchingParent`）。

## 兼容性

委派仅在 `backendRef {kind: HTTPRoute}` 时激活，而这种写法此前是非法配置——存量
路由行为完全不变。`allowedRoutes` 语义不受影响（默认仍是 `Same`）；子路由从不挂
listener，listener 策略对其不适用。

## 可观测性

摊平后的规则带有生成的规则 `name`（`<子ns>.<子名>.r<规则下标>`，子规则自己命名
则保留原名），日志与 status 中引用规则名即可定位所属子路由。
