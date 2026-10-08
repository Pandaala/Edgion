# GraphQL 本地错误

GraphqlGuard、GraphqlSafelist、GraphqlRateLimit 和 GraphqlOperationRouter 自身拒绝 HTTP 请求时，
返回固定 JSON，Content-Type 为 application/json：

```json
{"errors":[{"message":"GraphQL request rejected","extensions":{"code":"GRAPHQL_REQUEST_INVALID"}}]}
```

| HTTP 状态 | 错误码 | 原因 |
| --- | --- | --- |
| 400 | GRAPHQL_REQUEST_INVALID | 输入无效、形状或成本超限、操作无映射 |
| 403 | GRAPHQL_OPERATION_FORBIDDEN | 操作授权失败 |
| 405 | METHOD_NOT_ALLOWED | 方法不支持或通过 GET 执行 mutation |
| 413 | PAYLOAD_TOO_LARGE | 请求封装超过上限 |
| 415 | UNSUPPORTED_MEDIA_TYPE | 媒体类型或编码不支持 |
| 429 | RATE_LIMITED | 配额耗尽、身份缺失或 Redis Deny |
| 500 | INTERNAL_SERVER_ERROR | 内部求值失败 |
| 503 | GRAPHQL_ADMISSION_BUSY | 共享求值并发额度耗尽 |

消息固定，不包含查询、变量、SDL 或依赖错误。HTTP 状态保持原有含义。
限流响应沿用现有响应头：已知正等待时间时，Retry-After 向上取整；
单次成本超出容量、缺少身份或依赖失败等无法计算等待时间的拒绝不设置该头。
X-RateLimit 响应头继续以成本单位表示配额。

成功的上游响应保留自身响应头并正常流式转发。原生 gRPC 保留自身状态和响应头映射。
GraphQL 插件执行之外的配置无效、请求体捕获或代理失败沿用现有错误响应，
不属于这里的 JSON 合约。普通 RateLimitRedis 保留其配置的消息格式。
