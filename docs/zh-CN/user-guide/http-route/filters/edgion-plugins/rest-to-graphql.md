# REST-to-GraphQL

`RestToGraphql` 把已有 GraphQL 上游包装成普通 HTTPRoute 接口。管理员配置固定
query/mutation 和变量映射，客户端提供变量值。响应保留上游的 `data`/`errors`
结构，并按现有链路流式转发。

## 示例

下面的配置把 `GET /users/42?n=3` 转成发往 `/graphql` 的 JSON POST。
HTTPRoute 负责入口匹配和后端声明，CtxSet 提取用户 ID，适配插件生成请求。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: user-graphql
  namespace: default
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: user_id
            from: { type: path }
            extract: { regex: '^/users/([^/]+)$', group: 1 }
    - type: RestToGraphql
      config:
        query: |
          query User($id: ID!, $n: Int! = 2) {
            user(id: $id, limit: $n) { id name }
          }
        operationName: User
        path: /graphql
        variables:
          - name: id
            from: { type: ctx, name: user_id }
          - name: n
            from: { type: query, name: n }
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: user-rest
  namespace: default
spec:
  parentRefs:
    - name: public-gateway
  hostnames: [users.example.com]
  rules:
    - matches:
        - path: { type: PathPrefix, value: /users }
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: user-graphql
      backendRefs:
        - name: user-graphql-service
          port: 8080
```

引用的 Gateway 和 Service 需要已存在，Service 必须已经提供 GraphQL 接口。
插件不会生成 SQL 或搜索引擎查询。

## 配置

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `query` | string | 必填 | 固定 query/mutation，最多 16KiB，复用规范 GraphQL 解析器 |
| `operationName` | string | 省略 | 多 operation 文档需要指定选择器 |
| `path` | string | `/graphql` | 上游路径，最多 2048 字节，不能包含 query、fragment 或 authority |
| `variables` | list | `[]` | 最多 64 个映射，目标必须是选中 operation 声明的变量 |
| `variables[].name` | string | 必填 | GraphQL 目标变量名 |
| `variables[].from` | KeyGet | 省略 | 复用 ctx/header/query/path 等本地来源，与 bodyPointer 二选一 |
| `variables[].bodyPointer` | string | 省略 | 当前严格 JSON 请求体的 RFC6901 指针，空指针选择整个 JSON 值 |
| `variables[].format` | `text` / `json` | `text` | 字符串来源的解释方式；bodyPointer 本身提供原生 JSON，需要保持 text |

简单变量直接复用 KeyGet，常量及复杂提取/转换用前置 CtxSet/DSL 准备。
`path` 读取当前完整的规范化路径，不是命名路径参数。query 取值观察原始入口
query；header/ctx/body 观察插件执行时的状态，路径提取时机由前置插件决定。

String/ID 文本保留为字符串，Int/Float/Boolean 文本按严格 JSON 标量解析，
Int 限制为有符号 32 位。列表要求 JSON 数组，不做单值转列表。
`format: json` 可从 ctx/header/query 读取 JSON 对象或带引号的 JSON 字符串。
bodyPointer 保留原生类型。所有变量包装层的非空约束会校验；自定义 enum、
input-object、scalar、完整 SDL 和固定默认值的语义由上游校验。

缺失的可空或有默认值变量不写入 envelope，由上游应用默认值。没有默认值的
非空变量缺失、或向非空变量显式提供 null，会拒绝请求。每个值最多 16KiB
文本、4096 个节点、深度 32；每层数组最多 32 项，类型包装最多 8 层。
生成的完整 envelope 继续受规范 GraphQL 64KiB、结构、解析器及工作量限制。

## 请求与保护顺序

原始客户端方法仅支持 GET/POST/PUT/PATCH/DELETE。GET 不能执行 mutation，
即使前置插件已改写方法；原始 GET 不能携带物理请求体。生成请求统一使用 POST
和 application/json，并消费全部 REST query。Content-Encoding 必须为 identity；
非空输入体即使未被变量使用，也必须符合 JSON 媒体类型并通过严格解析。
解码后重复的 query 名、非法 query 编码、映射 header 的重复值/非 UTF-8 值会拒绝。

执行顺序为 Global → Gateway → Route。先认证和准备 REST 变量，再执行适配器，
最后执行 Guard/Safelist/RateLimit/Router。成功的 GraphQL 检查会冻结共享请求体，
包括原始无 body 请求随后才创建的 replay owner；放在后面的适配器无法发布替换体。
失败或被条件跳过的检查不会冻结；最终检查之前允许多个适配器依次执行。
检查成功后，DSL/ProxyRewrite/Wasm 的请求体改写也会被共享 owner 拒绝。
query/URI/method/header 的后续改写仍需遵守可信插件链顺序；这不是通用传输封印。

OperationRouter 仍只能选择当前 HTTPRoute rule 声明的 backendRefs。
请求体使用共享 BodyStorage/replay，包括原始无 body 请求的注入及重试。
适配器不负责后端选择，也不缓存响应。

## 错误与热更新

适配器运行时拒绝使用[固定 GraphQL JSON 错误](graphql-errors.md)。无效输入/映射返回 400，
方法限制 405，超限 413，不支持的媒体类型/编码 415，计算准入耗尽 503，
无效顺序/发布返回 500。无效启用配置使用规范的插件配置错误 500 响应。
拒绝不会发起上游请求。
禁用条目保持惰性，无效启用配置失败关闭，有效更新后恢复；已有请求保持其
不可变配置代际。
