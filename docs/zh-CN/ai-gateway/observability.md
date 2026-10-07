# AI Gateway 可观测性

## Access log `genai` 账本

AI 请求会在结构化 access log 中增加顶层 `genai` 对象；非 AI 请求省略。完整 schema 见
[Access Log](../ops-guide/observability/access-log.md)。

最终汇总包含 `provider`、`model`、`streaming`、`usage_source`、`usage_estimated`、
可选的 `quota_status` 与 `ttft_ms`，以及有界的 `attempts` 数组。只有已知的上报或有效估算
用量才会包含 token 计数，以及配置定价后的估算成本。汇总中的数值描述最终尝试，不是所有尝试的
请求总消耗；各尝试记录在 attempts 中。业务配额终态结算与可选 token 指标计入所有可核算的
已派发尝试，包括重试。出现未知尝试时，不能把已知部分当作权威请求总量。

`usage_source` 为 `reported`、`estimated`、`unknown` 或 `notObserved`。未知用量省略
计数和成本，不表示零。`usage_estimated` 仅在 `estimated` 时为 true，因此 false 本身
不能证明用量由提供商上报。任一次尝试未知，或已观测与未观测尝试混合时，请求汇总为 unknown；
全部未观测时为 notObserved。即使请求汇总未知，已知尝试仍保留各自的账本数据。
`usageObservation: off` 是默认值，原生与转换尝试均记录 notObserved，不输出 token 计数或成本。
`provider` 显式启用有界观测；`injectUsageTracking` 仅控制请求注入。关闭观测时，转换器仍保留
客户端协议需要的 usage；RPM 和成功 TTFT 仍由现有尝试生命周期处理。

`model` 是实际用于上游派发的有效逻辑模型。它通常与客户端请求的已声明模型名称或别名一致；明确的 `modelRoutes` 改写或 quota
`Downgrade` 动作会将其替换为配置的回退模型。启用未知模型透传后，它也可能是最长 512 UTF-8 字节
的客户端可控字符串。应把它视为潜在的高基数 access-log 字段：不要直接提升为 Prometheus 标签，
也不要在下游分面中假设它来自固定目录。如需有界聚合维度，请在下游进行分桶。下文可选的 GenAI
计数器特意不包含模型标签。

| 边界 | 值 |
|------|----|
| 最大 attempt 条目 | 8 |
| 序列化 attempts 段 | 8 KiB |
| 截断标记 | `attempts_truncated: true` |

每个 attempt 可记录尝试序号、带 Namespace 的 EdgionBackend 键、提供商枚举、凭据别名、状态、固定失败
类别、接受的 Retry-After 秒数、TTFT、上报或估算 token，以及估算成本。

`credential` 始终表示非敏感配置别名，绝不是 Secret 数据。日志从不包含提供商 URL、解析后的 IP、
凭据、prompt、completion、工具参数或提供商错误体。

## 可选 Prometheus 计数器

`edgion_gateway_genai_tokens_total` 默认关闭，进程本地，Gateway 重启后清零。启用后仅用固定标签
（`direction`、`provider`、`usage_source`）聚合用量已知的上报/有效估算尝试。用于速率与趋势 — 不要把它当作
持久用量或计费来源。未知或未观测尝试不生成 token 样本，也不补零；另一尝试未知时，已知尝试仍只计一次。

关闭这个计数器不关闭 usage 观测或 access-log 核算。

以下是 Gateway YAML 配置片段：

```yaml
metrics:
  genai_tokens:
    enabled: true
```

详情：[Metrics](../ops-guide/observability/metrics.md) ·
[Metrics inventory](../ops-guide/observability/metrics-inventory.md)。

## 状态排障

| 信号 | 含义 |
|------|------|
| HTTPRoute `Accepted=False/UnsupportedValue` | AI 挂载无效、后端 kind 混用、不支持的 backend Filter，或重试次数过多，**且该 route 已无任何 rule 在服务**（见 [AI 规则拒绝](resilience.md#ai-规则拒绝)） |
| HTTPRoute `PartiallyInvalid=True/RuleInvalid` | 同样的 AI 挂载错误，但该 route 仍有其他 rule 在服务；消息中会给出 `(N of M rules active)` |
| HTTPRoute `ResolvedRefs=False` | 缺失、未授权或不支持的 backendRef |
| EdgionBackend `Accepted=False/Invalid` | Endpoint、池、模型或默认值无效 |
| EdgionBackend `ResolvedRefs=False` | 至少一个凭据 Secret 缺失、未授权或无效；另有可用凭据时继续服务 |
| EdgionBackend `PartiallyInvalid=True` | 可用子集已激活；检查有界的无效凭据别名 |

## Responses 的账单与终态

provider 观察读取 input_tokens、output_tokens 和 input_tokens_details.cached_tokens；reasoning 已包含在
output_tokens 中，不重复相加。JSON 要求文档完整有效且 status 为终态；SSE 仅从 response.completed、
response.incomplete、response.failed 的对应终态读取 usage，delta/done 和重复输出不重复计费。
JSON cancelled 可识别；不假定存在 response.cancelled SSE 事件。通用 error 事件仅记录固定失败类别。

usage 缺失、事件超限/损坏、编码不支持或文档截断时为 unknown，不以字符数补实际账单或成本。
status=completed 且 usage 缺失时可以确认模型成功，但没有数字账单。HTTP200 failed/incomplete/cancelled
不会成为凭据健康或自适应 TTFT 的成功样本。off 不安装 body parser，只保留传输 TTFT，不推断模型成功。
attempt 失败类别增加 provider_failed、provider_incomplete、provider_cancelled；后续传输截断优先，保守保留凭据预留。

## 原生 Messages 核算

Messages 的 provider 观测只采用厂商用量，输入包含缓存计数，迟到计数按累计水位处理。
off 保持 notObserved；不支持的版本/复合推理、终态用量缺失或流损坏、编码不可观测、未完成
保持 unknown，不用字符估算。厂商错误不会被后续 stop 覆盖为成功。
详见[原生边界](./providers.md#原生-messages-的边界)。
