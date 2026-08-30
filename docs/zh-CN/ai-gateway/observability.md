# AI Gateway 可观测性

## Access log `genai` 账本

AI 请求会在结构化 access log 中增加顶层 `genai` 对象；非 AI 请求省略。完整 schema 见
[Access Log](../ops-guide/observability/access-log.md)。

最终汇总包含 `provider`、`model`、`input_tokens`、`output_tokens`、可选 cache token 计数与
`estimated_cost_usd`、`streaming`、`usage_estimated`、可选 `quota_status` 与 `ttft_ms`，以及有界的
`attempts` 数组。

`model` 是实际用于上游派发的有效逻辑模型。它通常与客户端请求的已声明模型名称或别名一致；quota
`Downgrade` 动作会将其替换为配置的回退模型。启用未知模型透传后，它也可能是最长 512 UTF-8 字节
的客户端可控字符串。应把它视为潜在的高基数 access-log 字段：不要直接提升为 Prometheus 标签，
也不要在下游分面中假设它来自固定目录。如需有界聚合维度，请在下游进行分桶。下文可选的 GenAI
计数器特意不包含模型标签。

| 边界 | 值 |
|------|----|
| 最大 attempt 条目 | 8 |
| 序列化 attempts 段 | 8 KiB |
| 截断标记 | `attempts_truncated: true` |

每个 attempt 可记录尝试序号、带 Namespace 的 Service 键、提供商枚举、凭据别名、状态、固定失败
类别、接受的 Retry-After 秒数、TTFT、上报或估算 token，以及估算成本。

`credential` 始终表示非敏感配置别名，绝不是 Secret 数据。日志从不包含提供商 URL、解析后的 IP、
凭据、prompt、completion、工具参数或提供商错误体。

## 可选 Prometheus 计数器

`edgion_gateway_genai_tokens_total` 默认关闭，进程本地，Gateway 重启后清零。启用后仅用固定标签
（`direction`、`provider`、`usage_source`）聚合每次已派发尝试。用于速率与趋势 — 不要把它当作
持久用量或计费来源。

在 Gateway 配置中启用：

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
| EdgionBackend `ResolvedRefs=False` | 没有可用凭据 |
| EdgionBackend `PartiallyInvalid=True` | 可用子集已激活；检查有界的无效凭据别名 |
