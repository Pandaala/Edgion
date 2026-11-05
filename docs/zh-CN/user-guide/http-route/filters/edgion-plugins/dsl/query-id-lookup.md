# 使用 DSL 实现认证查询 ID 查找

这是应用配置示例，使用现有 HTTPRoute、Secret KeyAuth、DSL 和 LinkSys Redis。
ID 只定位查询文档，不授予权限，也不选择上游 URL；它不是内置注册服务、Apollo APQ
或自动安全列表。发布者和执行者使用不同凭据。

## 前提和完整配置

在 example 命名空间准备名为 app-gateway 的 TLS Gateway，域名为
graphql.example.com。部署带 app: graphql-api 标签、监听 8080 端口的 GraphQL
应用，并替换 Redis 地址、密码和两个 API 密钥。凭据由运维提供，不能自行从请求
中的租户标记推导权限。下面两个执行入口共用一条 HTTPRoute 规则和同一个后端。

插件顺序为认证、DSL 展开、GraphqlGuard、ResponseCache。所有 Global/Gateway
正文检查器也必须遵守先展开再检查的顺序；成功检查会冻结正文。签名验证通常在
改写前执行，并验证其签名对应的请求视图。需要文档内容的授权、安全列表、限流或
路由器在展开后、缓存前执行。缓存命中仍执行认证和现有请求配额。

Redis 前缀来自固定配置，客户端租户头和 namespace 字段不能选择其他命名空间。
发布权限仅分配给有权发布该应用文档的运维或 CI 身份。

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: example
---
apiVersion: v1
kind: Secret
metadata:
  name: query-redis-auth
  namespace: example
type: Opaque
stringData:
  password: replace-with-redis-password
---
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: redis-test
  namespace: example
spec:
  type: redis
  config:
    endpoints:
    - redis://redis.example.svc:6379
    auth:
      secretRef:
        name: query-redis-auth
    timeout:
      connect: 3s
      command: 2s
---
apiVersion: v1
kind: Service
metadata:
  name: graphql-api
  namespace: example
spec:
  selector:
    app: graphql-api
  ports:
  - name: http
    port: 8080
    targetPort: 8080
---
apiVersion: v1
kind: Secret
metadata:
  name: query-publisher
  namespace: example
type: Opaque
stringData:
  keys.yaml: |
    - key: "replace-with-publisher-key"
---
apiVersion: v1
kind: Secret
metadata:
  name: query-executor
  namespace: example
type: Opaque
stringData:
  keys.yaml: |
    - key: "replace-with-executor-key"
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: query-publisher
  namespace: example
spec:
  requestPlugins:
  - type: KeyAuth
    config:
      keySources:
      - type: header
        name: x-dsl-query-key
      hideCredentials: true
      upstreamHeaderFields: []
      secretGroups:
      - name: default
        secretRefs:
        - name: query-publisher
          namespace: example
  - type: Dsl
    config:
      name: query-publisher
      source: |
        if req.request_method != "POST" { return deny(405, "Method not allowed") }
        if req.content_type != "application/json" { return deny(415, "Unsupported media type") }
        let encoding = req.header("Content-Encoding")
        if encoding != nil && encoding != "identity" { return deny(415, "Unsupported media type") }
        if req.query_string != nil && req.query_string != "" { return deny(400, "Invalid input") }
        if req.body.byte_len() > 65536 { return deny(413, "Payload too large") }
        let input = json.read(req.body)
        if input == nil { return deny(400, "Invalid input") }
        let keys = json.keys(input)
        if input == nil || keys == nil { return deny(400, "Invalid input") }
        for key in keys {
          if key != "query" && key != "operationName" { return deny(400, "Invalid input") }
        }
        let query = json.get_string(input, "query")
        if query == nil || query == "" { return deny(400, "Invalid input") }
        let selection = json.get_raw(input, "operationName")
        let name = json.get_string(input, "operationName")
        if selection != nil && (name == nil || name == "") { return deny(400, "Invalid input") }
        let mut record = json.set_string("{}", "query", query)
        if selection != nil { record = json.set_string(record, "operationName", name) }
        let id = sha256(record)
        let result = redis.set("redis-test", "edgion:dsl:query:v1:example:" + id, record, 604800000)
        if result["ok"] == false { return deny(503, "Query store unavailable") }
        resp.set_header("Content-Type", "application/json")
        return deny(201, json.set_string("{}", "id", id))
      maxStringLen: 65536
      maxBodyScanPasses: 4
      errorPolicy: deny
      linkSysRefs:
      - namespace: example
        name: redis-test
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: query-executor
  namespace: example
spec:
  requestPlugins:
  - type: KeyAuth
    config:
      keySources:
      - type: header
        name: x-dsl-query-key
      hideCredentials: true
      upstreamHeaderFields: []
      secretGroups:
      - name: default
        secretRefs:
        - name: query-executor
          namespace: example
  - type: Dsl
    config:
      name: query-executor
      source: |
        if req.request_method != "POST" { return deny(405, "Method not allowed") }
        if req.content_type != "application/json" { return deny(415, "Unsupported media type") }
        let encoding = req.header("Content-Encoding")
        if encoding != nil && encoding != "identity" { return deny(415, "Unsupported media type") }
        if req.query_string != nil && req.query_string != "" { return deny(400, "Invalid input") }
        if req.body.byte_len() > 65536 { return deny(413, "Payload too large") }
        let input = json.read(req.body)
        if input == nil { return deny(400, "Invalid input") }
        let keys = json.keys(input)
        if input == nil || keys == nil { return deny(400, "Invalid input") }
        let by_id = req.path == "/graphql/by-id"
        for key in keys {
          if by_id {
            if key != "id" && key != "variables" { return deny(400, "Invalid input") }
          } else {
            if key != "query" && key != "operationName" && key != "variables" { return deny(400, "Invalid input") }
          }
        }
        let variables = json.get_raw(input, "variables")
        if variables != nil && variables != "null" && json.keys(variables) == nil { return deny(400, "Invalid input") }
        let mut record = "{}"
        if by_id {
          let id = json.get_string(input, "id")
          if id == nil || regex_find(id, "^[0-9a-f]{64}$") == nil { return deny(400, "Invalid input") }
          let result = redis.get_bounded("redis-test", "edgion:dsl:query:v1:example:" + id, 65536)
          if result["ok"] == false {
            if result["error"] == "result_too_large" || result["error"] == "invalid_response" { return deny(502, "Invalid query mapping") }
            return deny(503, "Query store unavailable")
          }
          if result["value"] == nil { return deny(404, "Query mapping not found") }
          if sha256(result["value"]) != id { return deny(502, "Invalid query mapping") }
          record = json.read(result["value"])
          if record == nil { return deny(502, "Invalid query mapping") }
          let stored_keys = json.keys(record)
          if record == nil || stored_keys == nil { return deny(502, "Invalid query mapping") }
          for key in stored_keys {
            if key != "query" && key != "operationName" { return deny(502, "Invalid query mapping") }
          }
          let query = json.get_string(record, "query")
          let selection = json.get_raw(record, "operationName")
          let name = json.get_string(record, "operationName")
          if query == nil || query == "" || (selection != nil && (name == nil || name == "")) { return deny(502, "Invalid query mapping") }
        } else {
          let query = json.get_string(input, "query")
          let selection = json.get_raw(input, "operationName")
          let name = json.get_string(input, "operationName")
          if query == nil || query == "" || (selection != nil && (name == nil || name == "")) { return deny(400, "Invalid input") }
          record = json.set_string("{}", "query", query)
          if selection != nil { record = json.set_string(record, "operationName", name) }
        }
        let mut envelope = record
        if variables != nil { envelope = json.set_raw(record, "variables", variables) }
        json.set_body(envelope)
        req.set_header("Content-Type", "application/json")
        req.set_uri("/graphql")
        return next()
      maxStringLen: 65536
      maxBodyScanPasses: 4
      errorPolicy: deny
      linkSysRefs:
      - namespace: example
        name: redis-test
  - type: GraphqlGuard
    config:
      maxDepth: 8
      maxFields: 64
      maxAliases: 16
      maxRootFields: 16
  - type: ResponseCache
    config:
      maxTtl: 60s
      graphqlQueries: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: query-publisher
  namespace: example
spec:
  parentRefs:
  - name: app-gateway
    namespace: example
  hostnames:
  - graphql.example.com
  rules:
  - matches:
    - path:
        type: Exact
        value: /graphql/operations
    filters:
    - type: ExtensionRef
      extensionRef:
        group: edgion.io
        kind: EdgionPlugins
        name: query-publisher
    backendRefs:
    - name: graphql-api
      port: 8080
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: query-executor
  namespace: example
spec:
  parentRefs:
  - name: app-gateway
    namespace: example
  hostnames:
  - graphql.example.com
  rules:
  - matches:
    - path:
        type: Exact
        value: /graphql
    - path:
        type: Exact
        value: /graphql/by-id
    filters:
    - type: ExtensionRef
      extensionRef:
        group: edgion.io
        kind: EdgionPlugins
        name: query-executor
    backendRefs:
    - name: graphql-api
      port: 8080
```

## 发布和执行

```bash
# Register with a publisher credential. Copy the returned id into QUERY_ID.
curl -i https://graphql.example.com/graphql/operations \
  -H 'x-dsl-query-key: replace-with-publisher-key' \
  -H 'Content-Type: application/json' \
  --data '{"query":"query User($id: ID!) { user(id: $id) { name } }","operationName":"User"}'

QUERY_ID='<returned-id>'
curl -i https://graphql.example.com/graphql/by-id \
  -H 'x-dsl-query-key: replace-with-executor-key' \
  -H 'Content-Type: application/json' \
  --data "{\"id\":\"${QUERY_ID}\",\"variables\":{\"id\":\"42\"}}"

curl -i https://graphql.example.com/graphql \
  -H 'x-dsl-query-key: replace-with-executor-key' \
  -H 'Content-Type: application/json' \
  --data '{"query":"query User($id: ID!) { user(id: $id) { name } }","operationName":"User","variables":{"id":"42"}}'
```

发布成功返回 201 和稳定存储 JSON 字节的 SHA-256 ID。脚本先解码 query 和可选的
operationName，再按 query、operationName 的固定顺序使用 JSON setter 构造记录。
请求空白或字符串的等价转义不会改变文档内容和 ID；改变固定操作选择会产生不同 ID。
variables 属于每次调用，不存入注册记录。

operationName 可以省略，存在时必须为非空字符串；null、空值和非字符串均拒绝。
只有一个命名或匿名操作时，可以省略选择。多个操作缺少固定名称时，执行阶段由
规范 GraphqlGuard 拒绝。发布仅检查 JSON 形状与限额，不验证 GraphQL 语法、SDL
或后端业务。无效文档和不存在的选择会在访问源站前拒绝。按 ID 调用不能覆盖
operationName，即使提交相同名称也不允许；混合 ID/query 和未知字段也拒绝。

省略 variables 保持省略，显式 null 保持 null；对象变量保留嵌套值和数字原始字节。
字符串由 json.set_string 转义，变量由 json.set_raw 保留类型。递归重复键和转义
别名拒绝，不拼接 GraphQL 语法、不手工转义 JSON。完整信封只通过一次原子
json.set_body 发布；Content-Length 和 Transfer-Encoding 继续由 Gateway 管理。

## 过期、撤销和错误

redis.set 的 TTL 为七天，即 604800000 毫秒。重复发布可重置 TTL，读取不续期。
授权的 Redis 工具删除精确的前缀加 ID 键即可撤销映射。每次缓存查找前都重新读取
映射，缓存不能绕过删除、过期或损坏。凭据通过现有 Secret 热更新撤销或替换。

| 条件 | 结果 |
| --- | --- |
| 缺失、错误或跨角色凭据 | DSL/Redis 前 KeyAuth 401 |
| 原始方法不是 POST，即使被可信插件改写 | 405 |
| 不支持的媒体类型或编码 | 415 |
| JSON、重复/未知字段、选择、ID 或变量形状无效 | 400 |
| 输入超过 65,536 字节 | JSON 解析前 413 |
| 映射缺失、过期或撤销 | 404 |
| 映射损坏、超限或 UTF-8 无效 | 固定 502 |
| Redis 缺失、不可用或超时 | 固定 503 |
| 结构/累计工作限额或正文发布失败 | 现有固定 DSL/BodyAccess 500 |

仅支持 POST application/json、空 URL 查询串以及缺失/identity Content-Encoding。
输入、存储记录和重建输出各自受 64 KiB 字节上限、深度 32 和 4,096 JSON 节点限制。
重复解析与构造共用 VM 的 16 MiB 工作预算，物理正文和扫描预算独立适用；小于字节
上限的文档仍可能耗尽其他限额。redis.get_bounded 不会成功返回截断结果，但转换
上限不约束 redis-rs 在此之前聚合的 Redis 帧。错误不反射查询、变量、凭据或依赖细节。

## 响应缓存

仅在源站明确返回有限的公开可缓存响应时启用 [ResponseCache](../../../../../../en/user-guide/http-route/filters/edgion-plugins/response-cache.md)，
例如 Cache-Control: public, max-age=60。该公开契约不能返回个性化数据，否则移除
ResponseCache 或标记 no-store。本例采用受支持的仅请求头 KeyAuth 配置并移除凭据；
它不提供认证响应分区接口。

只有规范解析证明的 GraphQL query 才符合缓存条件。mutation、subscription、SSE、
增量/流式响应、不支持的正文变换、后端插件链、Set-Cookie、private/no-store 均绕过。
HTTP 200 不代表 GraphQL 没有错误；显式源站契约决定可缓存性，不增加完整响应解析。

完整请求与 ID 请求仅在最终信封、剩余请求头和路由/后端/策略身份一致时共享缓存。
脚本统一构造这两种请求，变量对象字节不同仍可能未命中。TTL 不保证 mutation 后
业务数据立即失效；需要即时新鲜度时使用短 TTL 或禁用缓存。主响应继续流式转发，
不会增加 DSL 响应正文 API 或响应 BodyStorage。
