# GraphqlSafelist

GraphqlSafelist 校验静态白名单中的完整 GraphQL 文档及实际选中的操作。
放在 requestPlugins 中，认证及所有请求改写之后；后续插件不得改写已放行的文档。

```yaml
type: GraphqlSafelist
config:
  operations:
    - query: |
        query Users($first: Int!) {
          users(first: $first) { id name }
        }
    - query: |
        query Read { ping }
        mutation Write { update }
      operationName: Read
```

客户端仍须通过标准 GET 或 JSON POST 发送完整 query。空白、注释和无意义逗号
可以不同；其余 token 以长度分隔后计算 SHA-256。字面量、字符串及块字符串的
原始字节、变量定义及默认值、别名、指令、片段和定义顺序必须一致。
不同转义写法或重排定义会被视为不同文档。运行时变量值可以变化。

白名单还校验实际选中的 query/mutation 类型和名称。注册项只有一个操作时可省略
operationName；客户端可显式选择该名称，也可省略。多操作文档必须指定注册的操作。
若要允许其中两个操作，使用同一完整文档注册两项、分别选择操作。
上述 Read 注册不会允许执行 Write，也不能用同名 Read 携带另一份查询绕过白名单。
匿名操作需要注册匿名文档。

支持 1..128 项，单份源码最多 64KiB，总源码最多 512KiB，选择器最多 256 字节。
重复的规范化文档及选中操作组合、无效文档会导致配置拒绝。
请求仍受共享 64KiB 封装、解析器、token、递归、工作量及四个求值并发额度约束。
指纹计算继续使用解析时的工作量计数。未知文档或未注册操作返回 403；
请求解析失败沿用 400/405/413/415。见 [JSON 错误合约](graphql-errors.md)。

放行后保留原请求字节及 URL，不需要 SDL 或 Redis。不支持动态注册、只传 hash 的
APQ、响应缓存或 GraphQL 执行；后端仍负责完整 GraphQL 校验。
形状保护和配额可组合 GraphqlGuard、GraphqlRateLimit。

条件和策略 bypass 可跳过检查。有效更新替换不可变白名单，无效白名单配置失败关闭。
若一次整资源更新同时撤销白名单并引入无效 DSL/WAF，现有整资源保留策略可能继续使用
上一版白名单。需要独立撤销时，将白名单放在单独的 EdgionPlugins 资源中。
