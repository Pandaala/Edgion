# 配额与成本

tokenQuota 要求 `usageObservation: provider`。观测默认 off；关闭时记录 notObserved，不发布
token 计数和成本。下文的观测与结算说明适用于显式启用 provider 观测的入口。

## Token 配额

可选的 AiProxy `tokenQuota` 基于 Redis，并按请求身份分桶。省略时不执行配额 Redis 操作。

`tokenQuota` 需要 `redisRef`、显式 `key` 数组，以及 1–8 个 limit。Limit 使用
`scope: identity` 或 `scope: identityModel`，外加 `tokens` 与 `window`。默认值为
`onKeyMiss: Deny`、`onRedisFailure: Deny`、`flushTokens: 100`、`flushInterval: 1s`、
`requestTimeout: 500ms`。

业务配额检查已累计的消耗，准入时不预留整次请求的输入或最大输出。运行中按输出观测分批刷新，
终态按所有已派发尝试的可核算输入与输出结算，包括重试消耗；不是只计算最后一次成功尝试。
提供商以非 2xx 状态回应的尝试（被重试的 429/5xx，或最终的 4xx/5xx）只计提供商自身上报的用量，
未上报时计 0 token、0 成本。中间层超时（504、524）仍视为不确定，保持原有的估算或 unknown 行为。
并发请求和刷新延迟允许超额，不能承诺严格的零超额预算。凭据 TPM 是独立的最大消耗预留机制，
见[凭据与限额](./credentials-and-limits.md#预留语义)。

桶归属由 AiProxy 条目的 Namespace、EdgionPlugins 名称及条目 alias（缺省用条目序号）决定。
多个 Route 引用同一条目会共享桶；独立条目即使配置相同也不共享。`key: []` 表示该条目内的
全局身份桶，不表示所有网关条目共享；当前没有独立的共享预算 ID 字段。

`identityModel` 使用准入时的有效逻辑模型名：默认保留客户端名称/别名，显式 `modelRoutes`
改写则先改变它。不同别名通常拥有独立桶，即使映射到同一 provider model。
每个模型最多配置 32 个别名模式，但 `prefix*` 可匹配许多不同客户端名称，不能据此推导
最多 33 个配额桶。需要跨名称总额度时使用同一条目内的 `identity` scope；使用 `Downgrade`
时只能配置 `identityModel`，因此要把别名及通配符带来的分桶计入策略。

以下是 AiProxy `config.tokenQuota` 内的动作片段。完整资源结构见[快速开始](./quick-start.md#完整示例)；
启用配额前需配置可用的 LinkSys Redis、`redisRef`、可信身份 `key` 和 `limits`。选择一种动作：

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

活跃请求持续向请求开始时的窗口记账，即使该窗口已经结束。没有新增输出单元时也会独立续租。
共享用量桶保留各次更新要求的最晚过期时间，一个请求结束不能缩短其他活跃请求所需的保留期；
结束请求的标记独立过期。

Redis 内存规划需要计入同时保留的多个窗口。每个共享桶只存一个用量计数，每个已记账请求另有一个标记。
活跃更新要求桶至少保留到请求开始时窗口的结束时间再加 60 秒，同时覆盖本次更新起算的
活跃租约（最大 HTTP 请求时长加 `tokenQuota.requestTimeout` 再加 60 秒）。
最终更新要求保留到该窗口清理截止时间与“本次更新加 `tokenQuota.requestTimeout` 再加 60 秒”中的较晚者。
每次更新都保留桶已有的更晚过期时间，因此最后一个活跃请求结束后桶仍可能继续存在；
停止更新后会按剩余 TTL 正常过期。
系统没有固定的全局内存上界，需要按身份和模型数量、limit 数量、窗口大小、请求时长及请求速率规划。
短窗口与长请求时长组合可能同时保留多个旧窗口桶。这些键保存记账状态，不保存响应正文。

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

以下解码降级与两次 malformed 容忍策略适用于 OpenAI/OpenAICompatible 被动 usage 观测。
Anthropic 转换器需要可转换的事件，会在第一次非法事件时按自身规则失败，不享有该容忍策略。

流中途执行依赖于解析上游的 SSE 帧结构：输出单元是从解析出的事件里数出来的。如果某个**未终止**的
事件超过了解码器 1 MiB 的上限，解码器会对该流的剩余部分放弃解析，此后再没有任何事件能被计数。

在 `onExceeded: Reject` 下这会 **fail-close** —— 帧结构已经无法解析的流，就是成本已经无法约束的流，
因此网关会结束它，而不是任由上游在无人计量的情况下继续生成计费 token。注入的事件复用准入路径在
"无法执行配额"时渲染的同一信封——
`{"error":{"message":"Token quota enforcement is temporarily unavailable.","type":"api_error","code":"token_quota_unavailable"}}`。

适用的门槛与上文相同：仅对配置了 `tokenQuota` 且 `onExceeded: Reject` 的**流式**请求生效。没有配额的
OpenAI 被动观测请求不会因该帧结构问题被截断；可用的字符估算可能只覆盖已解析前缀，精度会降低。
若其他观测错误导致用量 unknown，则按未知用量结算。1 MiB 是网关的检查边界，不是提供商协议保证。

无效 UTF-8 只会污染并丢弃包含它的那一条完整 SSE 事件。Edgion 会容忍一个响应中的第一条此类事件，但
计数会在整个响应内累计：中间出现合法事件不会重置计数；第二条 malformed 事件会在相同的“流式请求 +
已配置 `tokenQuota` + `onExceeded: Reject`”门控下 fail-close。无配额请求以及使用 `Throttle` 或
`Downgrade` 的路由不会因此终止。客户端仍只会收到上文通用的 `token_quota_unavailable` 信封，错误说明
不会暴露 malformed provider 字节或其他内部细节。

需要告警的信号：

| 错误诊断 | 含义 |
|----------|------|
| `AiQuotaOverLimitTerminated` | 一个流式 AI 响应因配额执行被中途截断——这是一个明确的执行动作，不是上游故障。 |
| `AiQuotaErrorEventDropped` | 截断前注入的错误事件未能放入单批次的发送预算，客户端只看到了裸截断，没有看到诊断事件。 |
| `AiSseDecodeDegraded` | 某个 AI 响应因单个未终止事件超过上限，SSE 解码放弃解析。可用字符估算可能只覆盖已解析前缀；用量或预算精度不能据此保证。 |
| `AiSseDegradedQuotaTerminated` | 一个流式 AI 响应因配额已无法被计量而被中途截断。与 `AiQuotaOverLimitTerminated` 的区别：后者意味着确实达到了限制，前者意味着限制已经无法被测量。 |
| `AiSseDecodeMalformed` | 至少一条完整 SSE 事件包含无效 UTF-8，因而未参与用量解析。无论是否启用配额执行，每个请求只记录一次。 |
| `AiSseMalformedQuotaTerminated` | 同一响应内累计出现第二条 malformed SSE 事件，触发流式 Reject 执行 fail-close；它既不表示实际超限，也不表示解码器永久降级。 |

## JSON 观测与未知用量

非流式 OpenAI Chat JSON 用量会增量提取，原始分片持续转发。观测器验证并丢弃内容字符串，
不保存整份响应等待 EOS 解析；仅在完整文档合法后发布上报用量。文档内重复选中字段和整个
usage/details 父对象的替换遵循最后一个值生效，与 SSE 累计快照的高水位合并分别处理。
合法完整的 OpenAI JSON 若缺少 usage，当前实现可使用输入估算与原始 JSON 字符数除以四向上取整
的输出估算；其中可能包含 JSON 包装或元数据，不能当作精确的模型 token 数。

观测采用固定边界：源字节与解码字节各 64 MiB、深度 64、解码 key 256 字节、原始 key
1,538 字节、数字文本 64 字节、词法事件 131,072、工作量 256 Mi 单位、保留堆容量 8 KiB。
这些是观测限制，不限制响应或下载大小。JSON 非法/不完整、预算耗尽、编码不支持或 token
算术溢出都会产生未知用量。被动观测继续转发原始字节，不把未知用量伪装成零或最终成本。

网关向上游发送 `Accept-Encoding: identity`，在转换或响应头修改前检查所有原始编码字段。
已准入的流式 tokenQuota + Reject 请求遇到编码响应时，在提交响应头前返回 HTTP 503 和
现有的 token_quota_unavailable JSON 错误体，不往压缩字节中插入明文 SSE。
Throttle 和 Downgrade 不新增这种硬拒绝；未触发 Reject 前置拒绝时，既有 Anthropic
转换器仍按其自身错误处理规则执行。

未知用量即使收到 EOS 或由网关截断，仍保留保守的凭据预留。业务配额成功完成保守刷新后以
unresolved 结束；Redis 失败或期限耗尽仍可能为 failed 或 timed_out。
刷新使用已观测、已确认或已发送的最高水位，避免退款一个可能已在 Redis 提交的请求；worker
仍会停止。观测水位为零不代表实际消耗为零。有效估算的客户端断开和网关截断仍按原规则处理，
传输歧义与用量质量是独立事实。检查 AiUsageObservationUnavailable；配置业务配额时，
算术溢出还使用 AiQuotaReconciliationFailed。已知尝试仍保留在 access log 中。

## 估算成本

在 EdgionBackend 上配置可选的 `models[].pricing`：

| 字段 | 说明 |
|------|------|
| `inputPerMillionUsd` | 普通输入 token 价格 |
| `outputPerMillionUsd` | 输出 token 价格 |
| `cacheReadInputPerMillionUsd` | 可选；默认等于输入价 |
| `cacheWriteInputPerMillionUsd` | 可选；默认等于输入价 |

配置定价且用量为上报或有效估算时，估算成本会出现在 access log 的 `genai` 汇总与 per-attempt 账本中。
未知/未观测用量省略成本。`CostWeighted` 选择还要求每个原本符合条件的候选都有定价与输出 token 上限。

持久计费与用量归属以 access log 为准 — 见 [可观测性](./observability.md)。

## Responses 的测量边界

Responses 复用累计配额与 Redis worker，只用厂商终态的 output 总量推进单调水位；delta 和重复输出不另计。
厂商可能只在结束时报告总量，因此不能保证逐 token 立即中止或并发零超卖。
终态 usage 不可靠时为 unknown，保留已收费配额与保守的凭据预留，不按零用量退款。
准入字符估计仅用于暂时容量判断，不是实际账单；远端历史、托管资源和隐藏 reasoning 无法从请求字符数测准。

被动观察保持损坏/超限原始字节透传并标记 unknown；主动 Reject 遇到损坏/超限 SSE framing 会按现有失败策略中止。
本地事件使用 Responses 的 type=error、code、message、param、sequence_number，不发送 Chat chunk 或 [DONE]。
序号取已验证厂商最大序号的安全后继，无有效序号时为零，溢出时不注入诊断事件。
该本地终止标记不承诺损坏/半个事件后的厂商重放顺序。上文 Chat 的字符估计和双坏事件容忍不适用于 Responses。

## 原生 Messages 配额

Messages 复用现有 quota worker 和终止所有者；输入缓存创建/读取计数只计一次，
累计 message_delta output 推进原有水位。off 不支持 tokenQuota、凭据 TPM、LowestUsage。
要求核算时，不支持的版本和已识别复合推理在上游发送前拒绝。unknown 保持既有保守预留与结算。
主动 Reject 使用原生 error event 关闭，Throttle/Downgrade 不获得该行为。
Token 估算只是临时准入输入，配置的价格估算不等于厂商完整账单。
