# 提供商

当前受管 AiProxy 入口使用 OpenAI Chat Completions。Edgion 把逻辑模型映射到所选后端的
提供商模型；客户端选择模型，显式的模型改写或 quota 降级策略才会改变该选择。

## 当前支持范围

| 客户端操作 | 后端 | 当前行为 |
|------------|------|----------|
| `POST /v1/chat/completions` | `OpenAI` / `OpenAICompatible` | Chat JSON 或 SSE；保留提供商字段，允许明确配置的请求修改 |
| `POST /v1/chat/completions` | `Anthropic` | Chat 转 Messages 请求；JSON 或 SSE 转回 Chat，仅支持下述转换子集 |
| `GET /v1/models` | Route 可达模型目录 | 本地生成目录；HTTPRoute 必须允许 GET 并匹配该路径 |
| `POST /v1/responses`、原生 `POST /v1/messages` | 任意受管后端 | 尚不支持 |
| Gemini native、Bedrock | 任意受管后端 | 尚不支持；后续范围 |

`api: openaiChat` 是操作识别的依据。入口接受有效路径 POST `/v1/chat/completions` 和本地
GET `/v1/models`；自定义路径需先经 URLRewrite 改写。其他受管操作返回固定的
`404 unsupported_endpoint`。Responses/Messages 枚举可识别，但在原生处理器实现前配置校验失败。
普通 HTTPRoute 到 Service 可独立代理不透明 HTTP 流量，不提供受管 AI 语义。

## 支持的提供商

| `provider` | 协议行为 | 默认 Endpoint |
|------------|----------|---------------|
| `OpenAI` | Chat 请求与响应 | `https://api.openai.com` |
| `OpenAICompatible` | Chat 请求与响应 | **必填** `endpoint` |
| `Anthropic` | Chat-to-Messages 转换；JSON 与 SSE 响应转回 Chat | `https://api.anthropic.com` |

Gemini、Bedrock、Vertex 与其他非 Chat API 不属于当前受管支持范围。

## OpenAI 与 OpenAI-compatible

请求与响应保持 Chat Completions 形态。`endpoint` 支持可选 path 前缀；仅在受控开发网络
设置 `allowInsecureHttp: true`。提供商扩展字段由后端解释，不代表网关理解其语义。
模型映射和 `injectUsageTracking` 可修改请求；quota Reject 也可中途终止 SSE，因此不要假设
启用这些策略后仍逐字节透传。

`onUnknownModel: passthrough` 要求 `Weighted` 或 `Priority`，且所有有效、权重为正的候选
都是 `OpenAI` 或 `OpenAICompatible`。显式模型改写仍要求目录中的 canonical 名称；
Anthropic 转换始终要求目录命中。

## Anthropic 转换

使用 `api: openaiChat` 与 `conversion: chatToAnthropic`，且候选全为 Anthropic，显式启用
现有 Chat-to-Messages 转换。原生入口省略 conversion，仅使用 OpenAI/OpenAICompatible 候选。转换支持文本消息、已声明的 function tools、tool calls/results、
`tool_choice`、`parallel_tool_calls` 和 `stop`。`n`、`temperature`、`top_p` 只接受省略、null 或中性值 1，
不支持任意采样参数。system 消息必须位于会话开头，最后一条必须是 user 或 tool result；不支持 developer role。
多模态内容、结构化输出、未知字段及其他不支持的组合会在本地拒绝，不会静默删除。
通过 `max_completion_tokens`、旧字段 `max_tokens` 或 `defaults.maxOutputTokens`
解析必需的输出 token 值；默认值只是回退，不是强制上限。

SSE 按有界事件增量转换。非流式 JSON 则在内存中保留一个受输入/输出字节、解析复杂度、
单次预留与进程共享预算约束的文档，完成验证和序列化后才提交最终响应头与响应体。
这仅用于所选转换，不是通用响应缓存；普通 HTTP、OpenAI 响应与 SSE 继续流式处理。
资源配置与默认值见[凭据与限额](./credentials-and-limits.md#非流式转换的资源限额)。

无效、截断或超限的转换在提交前返回固定 Chat 错误 `502 upstream_response_invalid`；
超时返回固定 `504 upstream_timeout`。准入容量不足在提供商派发前返回固定
`503 service_unavailable`，不会通过故障切换规避预算。原始 Anthropic 错误体不会发给客户端；
真正的提供商错误与插件明确拒绝按现有错误规则处理。

## 凭据安装

Edgion 移除客户端提供商认证 header，并在每次尝试时安装选中的 Secret 凭据。
应用不需要提供商密钥；客户端认证需另行配置普通认证插件。
