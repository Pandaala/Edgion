# 凭据与限额

每个 `EdgionAIService` 为同一提供商 Endpoint 拥有一个凭据池。凭据由 Secret 承载；状态与
access log 只出现配置别名。

## 凭据参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `credentials[].name` | String | **必填** | 唯一非敏感别名，1–64 个 ASCII 字符 |
| `credentials[].weight` | Integer | `1` | 加权轮转份额，1–1,000,000 |
| `credentials[].secretRef.name` | String | **必填** | 含 UTF-8 提供商凭据的 Secret，键为 `api-key` |
| `credentials[].secretRef.namespace` | String | Service Namespace | 跨 Namespace 需要 ReferenceGrant |
| `credentials[].limits.requestsPerMinute` | Integer | 无 | 可选分布式 RPM |
| `credentials[].limits.tokensPerMinute` | Integer | 无 | 可选分布式 TPM |

不要把提供商密钥写进 EdgionAIService、EdgionPlugins、HTTPRoute 或应用配置。

## Redis 要求

任一凭据配置了 RPM 或 TPM 时，必须设置 `credentialPool.redisRef`。`onRedisFailure` 默认为
`Deny`（fail-close）。`Allow` 在 Redis 不可用时显式绕过凭据限额。

## 预留语义

- **RPM** 在派发前立即预留 1 次请求。
- **TPM** 预留估算输入加上请求或 Service 的显式最大输出上限。无法解析输出上限时，TPM 限额凭据无效。
- 已知最终用量会冲正预留。
- 派发前取消会释放预留。
- 派发后超时、重置或其他歧义结果会保留完整预留直到 Redis 窗口过期。该预留保护限流，**不是**
  声称提供商消耗了全部预留 token。持久核算以 access log 为准。

## 轮转与 429 冷却

提供商 429 会先轮转凭据。有效的 Retry-After 会为该凭据创建有界冷却；不会驱逐兄弟凭据，也不会让
当前请求睡完整个提供商间隔。

AI 目标的 outlier / half-open 行为按凭据隔离 — 见 [弹性与故障切换](./resilience.md)。
