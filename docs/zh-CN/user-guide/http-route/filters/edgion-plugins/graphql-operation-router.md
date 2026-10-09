# GraphQL 操作路由

`GraphqlOperationRouter` 根据选中的 `operationType + operationName`，将完整请求转发到 HTTPRoute 声明的一个 Service 后端。它不执行 GraphQL、不拆分字段，也不转换 SQL 或 Elasticsearch 查询；后端需要提供 GraphQL 接口。

配置示例与完整限制见[英文文档](../../../../../en/user-guide/http-route/filters/edgion-plugins/graphql-operation-router.md)。插件放在 HTTPRoute rule 的 ExtensionRef 中，认证、请求改写和可选的 GraphqlRateLimit 放在前面，路由器放在最后。

所有映射及默认后端都必须出现在挂载插件的**同一条 rule** 的 backendRefs 中，包括当前请求没有选中的映射。其他 rule、其他 HTTPRoute 和集群中存在的 Service 都不能代替声明。省略 namespace 时使用 HTTPRoute 的命名空间；同一个 namespace/name 不能用不同端口重复声明。跨命名空间仍需 ReferenceGrant，Bypass 和条件不能使未声明的后端变合法。

POST 使用 JSON 对象；GET 使用 URL 参数且不能携带请求体，GET mutation 返回 405。多操作文档必须用 operationName 选择一个操作，名称区分大小写。匿名或未匹配操作只能使用显式 defaultBackendRef，没有默认值则返回 400。选中的后端失败时不会切换到其他后端，请求原文保持不变。

请求上限为 64 KiB，可配置更小值。解析还限制 JSON 深度、token、递归和工作量，并与成本限流共用四个非等待解析名额，繁忙返回 503。不支持订阅、批量、APQ、压缩、multipart、委派路由、全局/Gateway/backendRef 挂载或 AI/gRPC 转换链路。

插件与 backendRefs 热更新都会重新校验；非法 rule 会被 Controller 移除，合法兄弟 rule 保留，此时请求可能返回 404。Gateway 对仍匹配到的非法挂载返回 500。成本限流独立配置 SDL 与 Redis，发生后续路由失败时不会退还已扣成本。

插件自身的 HTTP 拒绝响应采用 [GraphQL JSON 错误合约](graphql-errors.md)。
