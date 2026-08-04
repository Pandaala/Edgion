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
