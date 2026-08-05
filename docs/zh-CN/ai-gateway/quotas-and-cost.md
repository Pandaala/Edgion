# 配额与成本

## Token 配额

可选的 AiProxy `tokenQuota` 基于 Redis，并按请求身份分桶。省略时不执行配额 Redis 操作。

`tokenQuota` 需要 `redisRef`、显式 `key` 数组，以及 1–8 个 limit。Limit 使用
`scope: identity` 或 `scope: identityModel`，外加 `tokens` 与 `window`。默认值为
`onMissingKey: Deny`、`onRedisFailure: Deny`、`flushTokens: 100`、`flushInterval: 1s`、
`requestTimeout: 500ms`。

选择一种带标签的 `onExceeded` 动作：

```yaml
onExceeded:
  type: Reject
```

```yaml
onExceeded:
  type: Throttle
  maxDelay: 250ms
```

```yaml
onExceeded:
  type: Downgrade
  model: chat-economy
```

- **Reject** — 超限时 fail-close。
- **Throttle** — 最多等待 1 ms–5 s（受剩余 HTTPRoute 请求截止时间约束），并重试准入一次。
- **Downgrade** — 要求所有 limit 使用 `identityModel`；替换逻辑模型后重新做后端资格与配额准入。

配额使用固定窗口，窗口在请求开始时按各 Gateway 实例墙钟选定。请保持 Gateway 节点时钟同步。

## 流中途终止

`tokenQuota` 的执行并不只在准入阶段生效。对于**流式**请求，若其配额桶在响应已经进行到一半时超限，
该响应会被中途截断：网关会先注入一条 SSE `error` 事件，使用与 429 准入拒绝相同的信封——
`{"error":{"message":"The token quota has been exceeded.","type":"rate_limit_error","code":"token_quota_exceeded"}}`
——随后结束响应。HTTP 状态码保持 `200`：因为响应头在截断发生前已经提交，无法再更改；客户端看到的是
注入的事件、随后是一次干净的流结束，而不是连接被直接断开。

该行为仅在 `onExceeded: Reject` 下生效。`Throttle` 与 `Downgrade` 是运维方明确表达的"不要对该用户
硬性失败"的策略，这一选择在已准入的流的剩余部分同样会被遵守——也就是说，在这两种动作下被准入的请求，
即使之后在流中途超限，也会照常运行到结束。

`over_limit()` 反映的是该身份**共享**的 Redis 配额桶状态，而不是被终止的这一个请求自身的消耗。因此同一
身份下的并发请求（例如用户打开了多个标签页）可能互相"误伤":哪个请求的下一个 chunk 恰好观测到配额桶已超限，
哪个请求就会被终止——这基本是随机的，与该请求自身实际消耗了多少配额无关；即使只有一个标签页把配额桶推过
了限制，三个标签页的流都可能因此被中断。

需要告警的信号：

| EdgionStatus | 代码 | 含义 |
|--------------|------|------|
| `AiQuotaOverLimitTerminated` | `999_12` | 一个流式 AI 响应因配额执行被中途截断——这是一个明确的执行动作，不是上游故障。 |
| `AiQuotaErrorEventDropped` | `999_13` | 截断前注入的错误事件未能放入单批次的发送预算，客户端只看到了裸截断，没有看到诊断事件。 |

## 估算成本

在 EdgionAIService 上配置可选的 `models[].pricing`：

| 字段 | 说明 |
|------|------|
| `inputPerMillionUsd` | 普通输入 token 价格 |
| `outputPerMillionUsd` | 输出 token 价格 |
| `cacheReadInputPerMillionUsd` | 可选；默认等于输入价 |
| `cacheWriteInputPerMillionUsd` | 可选；默认等于输入价 |

配置定价后，估算成本会出现在 access log 的 `genai` 汇总与 per-attempt 账本中。`CostWeighted`
选择还要求每个原本符合条件的候选都有定价与输出 token 上限。

持久计费与用量归属以 access log 为准 — 见 [可观测性](./observability.md)。
