# GraphQL 成本限流

`GraphqlRateLimit` 根据静态 GraphQL schema 估算查询成本，在转发前消耗 Redis
预算。它可以保护背后调用 Elasticsearch、ClickHouse 等数据库的 GraphQL 服务。
直接发送 ES DSL 或 ClickHouse SQL 的接口需要另行配置策略，本插件解析 GraphQL。

## 示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: graphql-budget
spec:
  requestPlugins:
    - type: GraphqlRateLimit
      config:
        schema: |
          type Query { users(first: Int = 10): [User!]! }
          type User { name: String! }
        costRules:
          - field: Query.users
            weight: 1
            multipliers: [first]
            defaultMultiplier: 100
        maxCost: 10000
        maxDepth: 20
        redisRef: default/shared-redis
        policies:
          - rate: 10000
            interval: 1m
            algorithm: SlidingWindow
```

通过 HTTPRoute 的 ExtensionRef 引用此资源。Redis 复用已有的 LinkSys Redis
连接。插件组合参见[执行顺序](../plugin-composition.md)。

## 成本与配置

每个操作先计 1 单位；每个字段的成本为
`weight + 各 multiplier 的乘积 × 子字段成本之和`。未配置的字段权重为 1，
倍率为 1。示例中 `{ users(first: 10) { name } }` 花费 12 单位；省略 `first`
时使用 SDL 默认值 10，也花费 12。

成本是运营者选择的估算值。仅凭 SDL 无法推断 resolver 的实际展开量、数据库
工作量或 CPU 消耗。返回对象、接口或 union（包括这些类型的列表）的 resolver
可配置分页倍率，并与后端的真实默认分页值保持一致。Scalar/Enum 返回字段
（包括标量和枚举列表）没有子字段成本，配置非空 `multipliers` 会被拒绝；
这些字段应使用保守的固定 `weight`。规则使用 `Type.field`，不受别名影响。重复字段和 fragment
引用逐次计费，`@skip` / `@include` 分支始终计费。接口和 union 对所有可能的
具体类型计算后取最大值；每个字段取完整接口规则与具体类型规则中的最大成本。
这些保守规则可能比实际执行收费更多。

| 配置 | 默认值与约束 |
|------|------------|
| `schema` | 必填，内联 SDL，不自动发现或拉取 schema |
| `costRules` | 空，最多 256 条 |
| `field` | 必填，存在的对象或接口字段 `Type.field` |
| `weight` | 1，范围 1..1,000,000 |
| `multipliers` | 空，最多 8 个不同的 Int 参数名；非空时返回类型须为对象、接口或 union |
| `defaultMultiplier` | 100，没有实参、变量或 SDL 默认值时使用 |
| `maxCost` | 10,000，范围 1..1,000,000 |
| `maxDepth` | 20，范围 1..32 |
| `redisRef`、`policies` | 必填，与 [RateLimitRedis](rate-limit-redis.md) 使用相同策略类型 |
| `key` | `[{type: remoteIp}]`；空列表使用全局预算 |
| `onRedisFailure` | `Deny`；显式 `Allow` 才允许 Redis 故障时放行合法查询 |
| `requestTimeout` | `200ms`，Redis 检查使用正数时长 |
| 插件入口 `body.maxBodySize` | 64KiB，可降低 POST 和 GET 的限制 |

身份来源全部缺失时拒绝；组合 key 中部分缺失时保留规范占位符。身份认证插件
应先运行。同一资源入口被多个路由引用时共享预算，各 Gateway 副本也共享；
不同入口使用独立预算。后端选择前计费一次，后端失败不退款。多个限流策略各自
原子执行，可能分别扣减。

## 请求和插件顺序

POST 接受一个 JSON 对象：必填非空 `query`，可选字符串或 null 的
`operationName`、对象或 null 的 `variables`、空对象或 null 的 `extensions`。
POST URL 不得带非空 query string。Content-Type 必须是单个 UTF-8 JSON 类型，
Content-Encoding 只能省略或为 identity。GET 使用严格解码的 URL 参数，不得有
body，且只能执行 query。文档有多个操作时必须提供 `operationName`。重复 key、
重复参数和未知入口参数均拒绝。

变量覆盖操作默认值，随后使用实际具体字段的 SDL 参数默认值。成本变量缺失但
声明为非空类型、显式 null、字符串、负数和非整数均拒绝。JSON 中的 1.0 可作为
整数，GraphQL 的浮点字面量不可用于倍率。倍率范围为 0..1,000,000，零按 1 计费。

大小限制分别作用于 POST body、编码后的 GET query 和解码后的 GET 参数。
还有固定的解析边界：4096 个 token（包括空白）、每个非空白 token 最长 256
字节、32768 次评估步骤。较长的业务字符串放入 variables。SDL 扩展、任意自定义指令和
接口继承暂不支持。完整 GraphQL 验证与执行仍由后端负责。subscription、WebSocket
消息、批量数组、上传、压缩 body 和仅带 persisted query hash 的请求暂不支持。

插件应放在所有认证和请求改写之后的 global/Gateway/route 请求插件中；BackendRef
的较晚阶段不支持，会拒绝请求。计费是运行位置的请求快照：之后的请求插件、
route/backend filter、DSL、ExtProc 不得再修改 query、body、方法或表示形式。
网关不封锁后续改写。入口 conditions、禁用和插件策略可以跳过整项保护。
`body.conditions`、`body.onReadFailure: failOpen` 配置无效。

## 结果

| 情况 | HTTP 状态 |
|------|----------|
| 合法且预算足够 | 原样代理，X-RateLimit 响应头表示成本单位 |
| 无效或不支持的查询、深度/工作量/maxCost 超限 | 400 |
| 不支持的方法、GET mutation | 405 |
| 请求大小超限 | 413 |
| 不支持的媒体类型或编码 | 415 |
| 预算耗尽、身份全部缺失、Redis 默认 Deny | 429 |
| 单次成本超过某项策略容量 | 429，不扣 Redis，也不返回 Retry-After |
| 全进程评估许可耗尽 | 503 |
| 无效配置、错误阶段、内部评估失败 | 500 |

无效的启用配置产生 `Accepted=False`，网关拒绝流量，热更新也如此；应用合法配置
后恢复使用新的成本模型。禁用入口不参与验证。插件不聚合响应，也不执行 resolver。
插件日志不记录查询和变量；已有的可选 `access_log_options.includeRawUri=true`
可能在访问日志中包含 GET 查询。

插件自身的 HTTP 拒绝响应采用 [GraphQL JSON 错误合约](graphql-errors.md)。

## 估算成本可见性

完整估算在现有插件访问日志中记录为 `cost=<数值>`，不记录查询、操作名称或变量值。
可选 `showQueryCost: true` 添加 `X-Graphql-Query-Cost`，默认关闭。该数值与传给加权限流器
的数值一致，但容量拒绝可能没有扣费，Redis 随后也可能失败，因此它不是扣费回执或实际数据库开销。
计算未完成时不报告成本，原有配额和 Retry-After 头保持不变。GraphqlRateLimit 始终执行限流；
业务阈值校准使用独立的 [GraphqlGuard 观察模式](graphql-guard.md)。

### 标准 SDL 元数据

共享成本编译器支持字段、参数、输入字段和枚举值上的 `@deprecated`。
`reason` 可省略，显式设置时必须是常量字符串；没有默认值的非空必填参数或
输入字段不能弃用。自定义 scalar 支持 `@specifiedBy(url: "https://example.com/scalar")`，
必须恰好提供一个常量字符串 URL；网关不会访问该 URL。

两者不可重复。null、非字符串、重复或未知参数、错误位置会导致配置无效。
元数据不改变权重、权限或估算成本，弃用字段仍正常计费。自定义指令、指令定义、
SDL 扩展、联邦注解、`@cost`、`@listSize`、`@oneOf` 仍不支持；元数据字符串
仍受现有 token、源文本和模型预算约束。

## 独立根字段预算

在必需的总预算之外，可增加最多 15 个命名根字段预算；总计最多 16 个配额、
64 个策略。在插件 config 内添加：

```yaml
rootQuotas:
  - name: users
    field: Query.users
    key: [{type: header, name: X-Tenant}]
    policies: [{rate: 2000, interval: 1m}]
responseQuota: total
```

field 使用 SDL 实际的 query/mutation 根类型名。未设置 key 时继承总预算身份，
显式 `key: []` 使用全局桶。没有出现的根字段不扣减。根预算计算完整选中子树，
包含字段权重和子字段乘数；别名及重复片段按现有保守语义累加。操作自身的
一个基础成本单位只计入总预算。同一根字段可以使用不同名称和身份建立多个
预算；query 与 mutation 共用一个根对象时，相同坐标的预算也跨操作类型共享。

先检查总预算，再按配置顺序检查有效根预算，共用已有 requestTimeout
（默认 200ms）。容量和身份错误在任何扣减前检查；后续根预算拒绝保留已经
完成的总预算扣减，不提供跨配额事务或退款。成功响应头默认来自 total，
responseQuota 可选择根预算；选中根字段缺席时不生成配额头。拒绝头来自实际
拒绝配额。查询成本仍是估算值，不代表扣减收据。已有 Redis 故障默认值和
固定 GraphQL 错误格式保持原合约。
