# 配额与成本

## Token 配额

可选的 AiProxy `tokenQuota` 基于 Redis，并按请求身份分桶。省略时不执行配额 Redis 操作。

`tokenQuota` 需要 `redisRef`、显式 `key` 数组，以及 1–8 个 limit。Limit 使用
`scope: identity` 或 `scope: identityModel`，外加 `tokens` 与 `window`。默认值为
`onKeyMiss: Deny`、`onRedisFailure: Deny`、`flushTokens: 100`、`flushInterval: 1s`、
`requestTimeout: 500ms`。

`identityModel` 使用客户端请求中的原始逻辑模型名。即使同一个 EdgionBackend 将 canonical
名称及其 alias 映射到同一个 provider model，它们仍使用不同的配额桶。一个模型最多可有 32
个 alias，因此同一身份最多可按 33 个已声明名称分别消耗一次所配置的额度。如需所有名称共享
一个总额度，请使用 `identity`，或不要为受配额控制的模型配置 alias。`Downgrade` 要求使用
`identityModel`，因此设置其 limit 时必须计入 alias 集合。

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

流式 provider usage 对象按累计快照处理。当 OpenAI-compatible provider 在多个 chunk 中提供
usage 时，Edgion 会保留各 token 分量观察到的最高值；后续较低或乱序的快照不会降低最终配额
结算、access log 用量或估算成本。OpenAI 通常只在最终 chunk 提供 usage 的行为不受影响。

## 流中途终止

`tokenQuota` 的执行并不只在准入阶段生效。对于**流式**请求，若其配额桶在响应已经进行到一半时超限，
该响应会被中途截断：网关会先注入一条 SSE `error` 事件，使用与 429 准入拒绝相同的信封——
`{"error":{"message":"The token quota has been exceeded.","type":"rate_limit_error","code":"token_quota_exceeded"}}`
——随后结束响应。HTTP 状态码保持 `200`：因为响应头在截断发生前已经提交，无法再更改；客户端看到的是
注入的事件、随后是一次干净的流结束，而不是连接被直接断开。

注入的事件前会带一个空行。上游在哪里切分 chunk 并不由网关决定，因此注入有可能落在一条只传了一半的
事件中间；这个空行会先把那条事件收尾，从而保证错误事件是作为独立事件被派发的，而不是被拼接到一段被
截断的 JSON 上、进而被解析器丢弃。对于合规的 SSE 客户端，这个额外的空行是不可见的——在没有缓冲内容
时，空行不会派发任何事件。

该行为仅在 `onExceeded: Reject` 下生效。`Throttle` 与 `Downgrade` 是运维方明确表达的"不要对该用户
硬性失败"的策略，这一选择在已准入的流的剩余部分同样会被遵守——也就是说，在这两种动作下被准入的请求，
即使之后在流中途超限，也会照常运行到结束。

`over_limit()` 反映的是该身份**共享**的 Redis 配额桶状态，而不是被终止的这一个请求自身的消耗。因此同一
身份下的并发请求（例如用户打开了多个标签页）可能互相"误伤":哪个请求的下一个 chunk 恰好观测到配额桶已超限，
哪个请求就会被终止——这基本是随机的，与该请求自身实际消耗了多少配额无关；即使只有一个标签页把配额桶推过
了限制，三个标签页的流都可能因此被中断。

### 当这条流已经无法被计量时

流中途执行依赖于解析上游的 SSE 帧结构：输出单元是从解析出的事件里数出来的。如果某个**未终止**的
事件超过了解码器 1 MiB 的上限，解码器会对该流的剩余部分放弃解析，此后再没有任何事件能被计数。

在 `onExceeded: Reject` 下这会 **fail-close** —— 帧结构已经无法解析的流，就是成本已经无法约束的流，
因此网关会结束它，而不是任由上游在无人计量的情况下继续生成计费 token。注入的事件复用准入路径在
"无法执行配额"时渲染的同一信封——
`{"error":{"message":"Token quota enforcement is temporarily unavailable.","type":"api_error","code":"token_quota_unavailable"}}`。

适用的门槛与上文相同：仅对配置了 `tokenQuota` 且 `onExceeded: Reject` 的**流式**请求生效。没有配额的
请求永远不会因为帧结构原因被截断——对它而言，降级一如既往只是把上报用量降级为字符估算而已。

1 MiB 这个上限的取值，是为了让正常的 provider 够不到它。一个正常的流式事件只有几百字节；只有把整个
回答作为单个事件发出的上游，或者干脆从不发出事件边界的上游，才会达到 1 MiB。

无效 UTF-8 只会污染并丢弃包含它的那一条完整 SSE 事件。Edgion 会容忍一个响应中的第一条此类事件，但
计数会在整个响应内累计：中间出现合法事件不会重置计数；第二条 malformed 事件会在相同的“流式请求 +
已配置 `tokenQuota` + `onExceeded: Reject`”门控下 fail-close。无配额请求以及使用 `Throttle` 或
`Downgrade` 的路由不会因此终止。客户端仍只会收到上文通用的 `token_quota_unavailable` 信封，错误说明
不会暴露 malformed provider 字节或其他内部细节。

需要告警的信号：

| EdgionStatus | 代码 | 含义 |
|--------------|------|------|
| `AiQuotaOverLimitTerminated` | `999_12` | 一个流式 AI 响应因配额执行被中途截断——这是一个明确的执行动作，不是上游故障。 |
| `AiQuotaErrorEventDropped` | `999_13` | 截断前注入的错误事件未能放入单批次的发送预算，客户端只看到了裸截断，没有看到诊断事件。 |
| `AiSseDecodeDegraded` | `999_14` | 某个 AI 响应因单个未终止事件超过上限，SSE 解码放弃解析。单独出现时损失的是计量精度而非正确性：上报用量回落为字符估算。 |
| `AiSseDegradedQuotaTerminated` | `999_15` | 一个流式 AI 响应因配额已无法被计量而被中途截断。与 `AiQuotaOverLimitTerminated` 的区别：后者意味着确实达到了限制，前者意味着限制已经无法被测量。 |
| `AiSseDecodeMalformed` | `999_16` | 至少一条完整 SSE 事件包含无效 UTF-8，因而未参与用量解析。无论是否启用配额执行，每个请求只记录一次。 |
| `AiSseMalformedQuotaTerminated` | `999_17` | 同一响应内累计出现第二条 malformed SSE 事件，触发流式 Reject 执行 fail-close；它既不表示实际超限，也不表示解码器永久降级。 |

## 估算成本

在 EdgionBackend 上配置可选的 `models[].pricing`：

| 字段 | 说明 |
|------|------|
| `inputPerMillionUsd` | 普通输入 token 价格 |
| `outputPerMillionUsd` | 输出 token 价格 |
| `cacheReadInputPerMillionUsd` | 可选；默认等于输入价 |
| `cacheWriteInputPerMillionUsd` | 可选；默认等于输入价 |

配置定价后，估算成本会出现在 access log 的 `genai` 汇总与 per-attempt 账本中。`CostWeighted`
选择还要求每个原本符合条件的候选都有定价与输出 token 上限。

持久计费与用量归属以 access log 为准 — 见 [可观测性](./observability.md)。
