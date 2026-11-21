# 提供商

受管 AiProxy 入口支持 OpenAI Chat Completions、原生 OpenAI Responses 和原生 Claude Messages。Edgion 把逻辑模型映射到所选后端的
提供商模型；客户端选择模型，显式的模型改写或 quota 降级策略才会改变该选择。

## 当前支持范围

| 客户端操作 | 后端 | 当前行为 |
|------------|------|----------|
| `POST /v1/chat/completions` | `OpenAI` / `OpenAICompatible` | Chat JSON 或 SSE；保留提供商字段，允许明确配置的请求修改 |
| `POST /v1/chat/completions` | `Anthropic` | Chat 转 Messages 请求；JSON 或 SSE 转回 Chat，仅支持下述转换子集 |
| `GET /v1/models` | Chat API Route 可达模型目录 | 本地生成目录；HTTPRoute 必须允许 GET 并匹配该路径 |
| `POST /v1/responses` | `OpenAI` | 原生 Responses JSON 或 SSE，要求显式 model |
| `POST /v1/messages` | `Anthropic` | 原生 Messages JSON/SSE，要求显式 model 与 max_tokens |
| Gemini native、Bedrock | 任意受管后端 | 尚不支持；后续范围 |

`api: openaiChat` 是操作识别的依据。入口接受有效路径 POST `/v1/chat/completions` 和本地
GET `/v1/models`；自定义路径需先经 URLRewrite 改写。其他受管操作返回固定的
`404 unsupported_endpoint`。`api: openaiResponses` 则只接受 OpenAI 的 POST `/v1/responses`。`api: anthropicMessages` 只接受 Anthropic 的 POST `/v1/messages`。
普通 HTTPRoute 到 Service 可独立代理不透明 HTTP 流量，不提供受管 AI 语义。

Chat Completions 请求命中响应侧 Guardrail 时，客户端必须设置 `"stream": true`。省略该字段或传入
`false` 都会在请求发往提供商之前收到 `400 unsupported_parameter`，两种 `failOpen` 设置均适用。
提供商最终返回 4xx 或 5xx 时，Guardrail 跳过评估和首窗口等待，继续走路由原有的 AI 错误处理流程。
例如，最终 429 可以携带网关按冷却策略校验并规范化的 `Retry-After`。

## 支持的提供商

| `provider` | 协议行为 | 默认 Endpoint |
|------------|----------|---------------|
| `OpenAI` | 原生 Chat 或 Responses 请求与响应 | `https://api.openai.com` |
| `OpenAICompatible` | Chat 请求与响应 | **必填** `endpoint` |
| `Anthropic` | 原生 Messages 或显式 Chat-to-Messages 转换 | `https://api.anthropic.com` |

Gemini、Bedrock 与 Vertex 尚不属于当前受管支持范围。

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
现有 Chat-to-Messages 转换。原生 Chat 入口省略 conversion，仅使用 OpenAI/OpenAICompatible 候选；原生 Messages 使用 Anthropic 候选。转换支持文本消息、已声明的 function tools、tool calls/results、
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

## 原生 Responses 的边界

配置 `api: openaiResponses`，省略 `conversion`，保持 `injectUsageTracking: false`。
默认关闭 usage 观察；配置 `usageObservation: provider` 后观察厂商原生 usage，不注入 Chat 的
`stream_options.include_usage`。原生 stream_options、input/output items、工具、reasoning 与扩展字段保持透传，
托管工具由厂商执行。受管路由要求显式 model；目前仅支持 POST `/v1/responses`，不支持检索/删除/取消、
后台创建（background=true）、WebSocket 或向其他厂商转换。

非 null 的 `previous_response_id` 或 `conversation` 要求路由只声明一个后端槽位、后端只声明一个凭据绑定。
未解析凭据和零权重后端槽位也计入原始声明数量；凭据权重本身不能为零。两个续接字段互斥。
续接请求拒绝改变厂商模型的 alias、任何 modelRoutes 配置、quota Downgrade，并禁止重试或自动切换主体/后端。
无状态请求保留正常选择能力，不新增会话注册表。跨厂商主体的 Secret 轮换仍须由运维保证身份一致。

AiGuard/Guardrail 目前仅理解 Chat 语义，与 Responses 在同资源启用时拒绝配置；跨作用域实际执行时也会拦截，
包括 URLRewrite 前已执行 AiGuard 的情况。disabled、条件跳过或 policy bypass 不触发运行时标记。
原生 guard 能力另行实现。

受管凭据安装还会移除 OpenAI-Organization / OpenAI-Project，包括后端 HeaderModifier 设置的值；
RequestId 不能以这两个保留 header 为目标在稍后恢复它们。它们属于厂商主体选择，不是普通扩展字段。
请使用 project-scoped key，并由运维保持主体固定；当前受管入口不支持 selector override。普通 HTTP 透传不受影响。

## 原生 Messages 的边界

配置 `api: anthropicMessages`，省略 `conversion` 和 `injectUsageTracking`；
EdgionBackend 使用 `provider: Anthropic`。只管理 POST `/v1/messages`，不管理
count_tokens、files、batches 等端点。要求显式 model 和非负整数 max_tokens；
零值原样保留，可用于缓存预热。原生 JSON/SSE、工具、签名、缓存控制和扩展字段原样传递；
无状态请求的显式模型映射只修改 model 值。

网关对每个元数据数组最多检查 4096 项，对整段对话最多检查 4096 个内容块，
包括工具结果中的嵌套块。超过任一上限后，网关按有状态请求的约束路由，并将用量标记为不可观测。
符合约束的路由仍可转发对话。启用令牌配额、按最低用量选择后端，或候选凭据设有每分钟令牌上限时，
网关需要核算用量，会在转发前拒绝请求。

原生用量记账接受解码后不超过 64 个 UTF-8 字节的非空 `stop_reason`，包括厂商新增的值。
SSE 记账还要求上游报告用量，并以 `message_stop` 完成事件序列。

保留 query 和所有有效 anthropic-beta 值；缺省 anthropic-version 使用
`2023-06-01`。目前仅对该版本支持厂商用量观测。其他版本在 off 下透传，在可选
provider 观测下透传并保持 unknown；要求核算的策略会在上游发送前拒绝。
清除客户端认证和 workspace selector，再安装 Secret 中的 Key。Key 应固定归属一个
workspace；有效 anthropic-user-profile-id 暂不支持。RequestId/RemoteIp 的晚到改写
不能覆盖受管身份或已认证的协议 header。

复用 container、thinking/redacted-thinking 历史、container upload 和文件引用
（包括 tool_result 内嵌内容）要求一个原始 backend slot、一个声明的凭据绑定，
模型保持不变，并禁止 modelRoutes、降级和重试。未解析凭据和零权重 backend slot 也计入。
Secret 轮转应维持相关账号/workspace/model，网关不证明对象授权、不建立 affinity 注册表。
未知 beta 标识原样透传，不承诺可跨凭据迁移。

AiGuard/Guardrail 组合在校验和实际执行时拦截。provider 观测使用有界增量解析，
不缓存完整响应。输入用量为 input_tokens 加 cache_creation_input_tokens 和
cache_read_input_tokens，细分字段不重复相加；SSE 迟到的原始计数按累计水位合并。
只有 start 中的占位值不足以证明最终用量，需要 message_delta output 和有效停止信息、
message_stop。缺失、畸形、超限、编码不可观测、流不完整及厂商错误均保持 unknown；
被动观测保留响应字节。主动 quota Reject 发原生 `event: error` 并关闭，
不发送 Chat 的 DONE；tool_use、pause_turn、max_tokens、refusal 属于正常停止。

已识别的 compaction/advisor/厂商 fallback 推理在 off 下透传，开启观测时保持 unknown；
要求核算时提前拒绝。响应 iterations 只接受同模型的 message 汇总，不重复相加子项。
配置价格仅估算 token 成本，不涵盖全部工具费、分层价格、混合缓存 TTL 或完整厂商账单。

参见 [Messages](https://platform.claude.com/docs/en/api/messages/create) 和
[流式协议](https://platform.claude.com/docs/en/build-with-claude/streaming)。
