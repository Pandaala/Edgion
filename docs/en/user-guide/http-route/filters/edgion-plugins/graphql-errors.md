# GraphQL local errors

GraphqlGuard, GraphqlSafelist, GraphqlRateLimit and GraphqlOperationRouter return fixed JSON for
their HTTP rejections, with Content-Type application/json:

```json
{"errors":[{"message":"GraphQL request rejected","extensions":{"code":"GRAPHQL_REQUEST_INVALID"}}]}
```

| HTTP status | Code | Cause |
| --- | --- | --- |
| 400 | GRAPHQL_REQUEST_INVALID | Invalid input, shape/cost violation or unmapped operation |
| 403 | GRAPHQL_OPERATION_FORBIDDEN | Operation authorization failure |
| 405 | METHOD_NOT_ALLOWED | Unsupported method or GET mutation |
| 413 | PAYLOAD_TOO_LARGE | Envelope exceeds its bound |
| 415 | UNSUPPORTED_MEDIA_TYPE | Unsupported media type or encoding |
| 429 | RATE_LIMITED | Quota exhausted, missing identity or Redis Deny |
| 500 | INTERNAL_SERVER_ERROR | Internal evaluation failure |
| 503 | GRAPHQL_ADMISSION_BUSY | Shared evaluation admission exhausted |

The message is always fixed; responses never include query text, variable values,
SDL or dependency errors. The HTTP status remains meaningful. Rate-limit rejection
headers use the existing limiter contract: positive waits have a rounded-up
Retry-After; capacity, missing-identity and dependency rejections have no
Retry-After when no wait is known. X-RateLimit headers retain cost-unit semantics.

Successful upstream responses retain their headers and stream normally. Native
gRPC retains its own terminal status/header mapper. Generic invalid-configuration,
request-body capture and proxy failures outside GraphQL plugin execution retain
their existing responses; they are not covered by this JSON contract. Ordinary
RateLimitRedis responses retain their configured message format.
