# AI Gateway

> **🔌 Edgion 扩展**
>
> Edgion AI Gateway 把 HTTPRoute hostname 变成兼容 OpenAI 的 LLM 流量入口。它不属于标准
> Kubernetes Gateway API。

受管入口通过 OpenAI Chat Completions、原生 Responses 创建 API 或 Claude Messages 调用 Edgion。Edgion 负责提供商 Endpoint、Secret 凭据、
模型目录、选择、重试、用量与成本核算。客户端不持有提供商 API Key。

## 内容导航

| 文档 | 说明 |
|------|------|
| [快速开始](./quick-start.md) | Secret → EdgionBackend → AiProxy → HTTPRoute → OpenAI SDK |
| [概念](./concepts.md) | HTTPRoute、EdgionBackend、AiProxy、EBTP 的职责划分 |
| [EdgionBackend](./edgion-backend.md) | 提供商 Endpoint、模型目录与类型化 AI 后端 |
| [路由与选择](./routing.md) | Weighted、Priority、LowestLatency、LowestUsage、CostWeighted |
| [凭据与限额](./credentials-and-limits.md) | 凭据池、RPM/TPM 预留、轮转与冷却 |
| [弹性与故障切换](./resilience.md) | 凭据优先重试、SSE 承诺、outlier half-open、EBTP |
| [提供商](./providers.md) | OpenAI / OpenAI-compatible 透传与 Anthropic 转换 |
| [配额与成本](./quotas-and-cost.md) | Token 配额动作与估算成本 |
| [可观测性](./observability.md) | Access log `genai` 账本与可选 token 指标 |

## 能力一览

- **统一 OpenAI 客户端面** — 同一 base URL 覆盖 OpenAI、OpenAI-compatible 与 Anthropic
- **类型化 AI 后端** — HTTPRoute `backendRefs` 直接引用 `edgion.io/EdgionBackend`
- **凭据隔离** — Secret 凭据池按权重轮转；RPM/TPM 走 Redis
- **智能选择** — 五种 `backendSelection`，AiProxy 不内嵌提供商列表
- **有界弹性** — 共享尝试上限、凭据优先轮次、不在流中途切换后端
- **用量与成本** — 持久化的 per-attempt access log 账本，外加可选进程本地 Prometheus 速率

当前支持 Chat JSON/SSE、原生 OpenAI Responses JSON/SSE，以及到 Anthropic 的有限 JSON/SSE 转换。原生
Messages 已支持，Gemini 与 Bedrock 尚未实现；能力边界见[提供商](./providers.md#当前支持范围)。
`api` 必填，选择 `openaiChat`、`openaiResponses` 或 `anthropicMessages`。默认原生转发；Anthropic 转换需显式声明 `conversion: chatToAnthropic`。
`usageObservation` 默认 `off`，`injectUsageTracking` 默认 false。提供商用量观测与请求注入分别配置。

## 相关文档

- HTTPRoute Filter 下的插件入口：[AI Proxy（插件短页）](../user-guide/http-route/filters/edgion-plugins/ai-proxy.md)
- AI 目标的后端流量策略：[熔断](../user-guide/http-route/backends/circuit-breaker.md)
- Access log 与指标：[Access Log](../ops-guide/observability/access-log.md) · [Metrics](../ops-guide/observability/metrics.md)
