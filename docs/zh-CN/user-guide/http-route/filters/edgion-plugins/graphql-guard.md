# GraphqlGuard

GraphqlGuard 不依赖 Redis 或 SDL，可独立限制 GraphQL 查询的深度、别名、根字段和总字段数。
放在 requestPlugins 中，认证和请求改写之后；后续插件不得再改动已经检查过的 operation。

```yaml
type: GraphqlGuard
config:
  maxDepth: 20
  maxAliases: 64
  maxRootFields: 32
  maxFields: 1024
  allowIntrospection: false
```

省略字段使用上述默认值，显式 null 关闭对应业务阈值。字段计数支持 0..4096，0 表示禁止该形状；
深度支持 1..32，根字段深度为 1。只统计选中的 operation，但重复 fragment、别名和 skip/include
各分支均保守计数。fragment 本身不增加字段深度。默认禁止 __schema 和 __type，别名和 fragment
不能绕过；__typename 允许并计数。默认 enforce 模式下超过阈值返回 400，原始请求保持不变。

可选 maxCost + schema + costRules 复用 [GraphqlRateLimit 的 SDL 成本模型](graphql-rate-limit.md)，
仍然不需要 Redis。成本含请求基础费用。schema 和非空 costRules 必须配合 maxCost，启用成本计算
时不得允许 introspection。maxDepth:null 只关闭形状深度阈值，成本引擎仍保留独立的 32 层上限。

复用严格 GET / JSON POST 和 64KiB 请求上限，以及共享的解析、工作量和并发限制。条件和策略可以
跳过插件，需要明确配置。后端仍负责完整 GraphQL 校验和字段授权；响应继续流式转发。
详见 [英文完整说明](../../../../../en/user-guide/http-route/filters/edgion-plugins/graphql-guard.md)。

插件自身的 HTTP 拒绝响应采用 [GraphQL JSON 错误合约](graphql-errors.md)。

## 根字段规则

可分别限制 query 和 mutation 的根字段，不需要 SDL：

```yaml
type: GraphqlGuard
config:
  rootFields:
    query:
      allow: [users, health, __typename]
      deny: [internalStatus]
    mutation:
      allow: [updateProfile]
```

策略或 allow 缺失、为 null 时不限制；allow: [] 禁止所有根字段。
deny 优先，同名同时出现在 allow/deny 时仍拒绝。每个列表最多 256 个不重复的
GraphQL 名称，每个 1..256 字节。名称只校验语法，后端仍负责完整 SDL 校验及授权。

所有被选中的根字段都要通过检查。别名按真实字段名匹配，片段不会改变根字段深度，
skip/include 的所有分支都会保守检查。嵌套字段不属于根字段规则。
实际选中的操作类型决定使用 query 还是 mutation 策略。__typename 同样受策略限制，
allow 无法覆盖 allowIntrospection: false。根字段策略拒绝返回 403，形状、成本和
introspection 拒绝仍返回 400；可选 SDL 成本计算保持原有语义。

仍需保证所有改写在检查之前完成、后续插件不再改写已放行文档。
若有效权限收紧和无效 DSL/WAF 改动在一次整资源更新中提交，现有保留策略可能
保留上一版权限。需要独立更新权限时，将 GraphqlGuard 放在单独的 EdgionPlugins 中。

## 观察模式和成本记录

设置 `mode: measure`，先观察业务阈值；确定配置后改回默认的 `mode: enforce`。
只有 maxDepth、maxAliases、maxRootFields、maxFields、maxCost 超限可以继续转发。
格式错误、SDL 计算错误、内省禁用、根字段权限和解析/工作量/并发等硬限制仍然拦截；
Safelist、Redis 限流等其他插件也继续执行。启用成本计算时，观察模式仍限制
估算成本最多 1,000,000、字段深度最多 32；无法完成计算的请求不报告精确成本。

现有插件访问日志记录完整形状计数及 `cost=<数值>`，不记录查询、操作名称或变量值。
`measure exceeded=<掩码>` 表示超过哪些阈值：深度 1、别名 2、根字段 4、总字段 8、成本 16；
数值相加，0 表示未超限，31 表示全部超限。

可选 `showQueryCost: true` 在响应中添加 `X-Graphql-Query-Cost`，默认关闭，
本插件需要同时配置 maxCost 和 schema。只报告完整估算；enforce 提前拒绝时可能没有该头。
它表示本地成本模型的估算单位，不代表数据库实际开销或 Redis 已经扣费。
多个插件开启时遵循现有响应头队列顺序；建议只启用一个成本头生产者。

### 标准 SDL 元数据

Guard 的可选成本模型与 [GraphqlRateLimit](graphql-rate-limit.md#标准-sdl-元数据)
共用元数据校验：支持标准位置的 `@deprecated` 和自定义 scalar 的 `@specifiedBy`。
元数据不改变成本或根字段权限。错误或不支持的元数据会在 Controller 和 Gateway
使配置无效；网关不会访问元数据 URL。
