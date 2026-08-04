# 提供商

客户端始终发送 OpenAI Chat Completions 请求。Edgion 把每个逻辑模型独立映射到所选后端的提供商模型。

## 支持的提供商

| `provider` | 协议行为 | 默认 Endpoint |
|------------|----------|---------------|
| `OpenAI` | 透传 | `https://api.openai.com` |
| `OpenAICompatible` | 透传 | **必填** `endpoint` |
| `Anthropic` | 流式转换到原生 Messages API | `https://api.anthropic.com` |

Bedrock、Vertex、多模态、beta 提供商与非 chat API 不在本表面范围内。

## OpenAI 与 OpenAI-compatible

请求与响应保持 OpenAI Chat Completions 形态。`endpoint` 支持可选 path 前缀。仅在受控开发网络
上设置 `allowInsecureHttp: true`。

`onUnknownModel: passthrough` 仅可用于采用 `Weighted` 或 `Priority` 的 OpenAI-compatible 候选。
Anthropic 转换始终要求目录命中。

## Anthropic 转换

对 `provider: Anthropic`，Edgion 把上游调用改写为原生 Messages Endpoint，并增量转换 JSON 与
SSE 响应。通过 `max_completion_tokens`、旧字段 `max_tokens` 或 `defaults.maxOutputTokens`
设置输出上限。不支持的字段会在本地失败，而不是被静默删除。主上游响应保持流式；原始提供商错误
体不会转发给客户端。

## 凭据安装

Edgion 会移除客户端提供商认证 header，并在每次尝试时安装选中的 Secret 凭据。应用不会获得提供商密钥。
