# 竞品能力吸收清单

调研日期：2026-08-03。范围：三家主线对标（Envoy AI Gateway / Kong AI Gateway /
Traefik Hub）**之外**的产品，找它们做到而三家没做到、且值得 Edgion 吸收的能力。

本文件是**决策记录**：每条注明来源、为什么三家没有、实现权重、落到哪个任务。
已吸收的条目在对应任务文件里有完整需求清单，这里只做索引与取舍理由。

## 基线（不重复列入）

三家已普遍具备、不算差异化：provider 翻译、凭证注入、token 限流/配额、
provider fallback、基础护栏、语义缓存（Kong/Traefik 有，Envoy v1.0 仍无）。
2026 年的新动向：Kong 3.14 加了 Agent Gateway 层（MCP/A2A）；
Traefik "Triple Gate" 加了多厂商可组合安全 pipeline（并行执行 + agent 友好的软拒绝）；
Envoy AI Gateway v1.0 把美元级成本管控与配额感知路由列在 roadmap（即尚未实现）。

## 已吸收

| # | 能力 | 来源 | 三家为何没有 | 权重 | 落点 |
|---|------|------|-------------|------|------|
| 1 | 虚拟 key + 组织/团队/用户多级预算与消费追踪 | LiteLLM | 三家都是无状态 CRD 代理，没有租户库；成本治理是它们共同最弱轴 | 重 | AI-2 §5 |
| 2 | Provider 凭证池 + 感知限流的路由（N 把 key，每把独立 tpm/rpm，调用前过滤、失败冷却） | LiteLLM | 三家路由的是后端，不是凭证池；LB 停在端点层 | 中 | AI-3 §6 |
| 3 | 基于实测遥测的路由（滚动 p50–p99、故障窗口剔除、价格反平方加权、请求级 sort 偏好） | OpenRouter（算法）+ LiteLLM（策略集） | 三家只有静态优先级/权重，不读实测分位数与价格 | 中 | AI-3 §7 |
| 4 | 预算动作不止拒绝：限速或降级到便宜模型 | TrueFoundry | 三家把配额当二值开关 | 轻 | AI-3 §8 + AI-2 §6 |
| 5 | 外部 guard 契约化（一个 evaluator 契约，Bedrock/Azure/自托管分类器可换） | AWS Bedrock ApplyGuardrail、Azure APIM llm-content-safety；marketplace 形态见 Portkey | 三家硬编码若干固定厂商集成，没有稳定契约，每加一家加一个插件 | 中 | AI-4 §6（**改变原设计方向**） |
| 6 | Prompt token 事前预估拦截（转发前估算，超配额直接拒） | Azure APIM `llm-token-limit` | 三家都从响应读 usage 事后计量 | 轻（但依赖分词器 → AI-5） | AI-4 §7 + AI-2 开放问题 |
| 7 | PII 感知的数据驻留路由（敏感 → 自托管模型；否则脱敏后走外部） | Wealthsimple 内部网关 | Traefik 做脱敏但不做路由后果 | 中 | AI-4 §8 |
| 8 | 非 chat 端点的统一治理（embeddings/audio/images/batch 同一套 key 与预算） | LiteLLM | 三家只翻译 chat(+embeddings)，其余端点用户绕过网关，治理漏 | 中偏重 | AI-2 §7 |
| 9 | 会话/追踪分组 + 可选载荷日志（工作流级成本与延迟） | Helicone（session 树）、Cloudflare（Logpush） | 三家出 metrics/OTEL 但无会话语义 | 中 | AI-2 §8 |

补充信号：Helicone 于 2026-03 被 Mintlify 收购转维护模式——独立观测代理正在被网关
吸收，佐证 #9 本就该长在网关里。

## 待评估（未纳入任何任务，需先有决策）

| 能力 | 来源 | 为什么先不做 |
|------|------|-------------|
| 版本化的声明式路由流程图（条件/百分比/预算节点，可草稿、即时发布、回滚） | Cloudflare Dynamic Routes、Portkey Configs | 形态很好，但与 Edgion 现有 CRD + GitOps 模型重叠；GitOps 已经提供版本与回滚，再加一层图编辑器可能是重复建设。若将来有非工程角色调路由的诉求，再重估 |
| 网关侧托管的 prompt 模板与版本（客户端只传变量与模板 ID） | Portkey、APISIX ai-prompt-template | 会把 prompt 生命周期管理引入网关，边界外扩明显；建议先观察用户是否真的要在网关做这件事 |
| 网关侧 RAG（网关查向量库并注入上下文） | APISIX ai-rag、Kong RAG Injector | 依赖向量能力（AI-5 才解锁），且把检索逻辑放进数据面会显著加重职责 |

## 明确否决

| 能力 | 来源 | 否决理由 |
|------|------|---------|
| 网关内小模型意图路由 + 函数调用（prompt → 直接调后端 API） | archgw / Plano（Katanemo） | 技术上确实独一份，但要求数据面旁边跑模型推理（1.5B–4B），与"精简 K8s 数据面"定位根本冲突。记录理由以免反复讨论 |
| Marketplace 式 BYOK 与多家托管同一开源模型的竞价路由 | OpenRouter | 只在"一个模型有很多竞争性托管方"的 SaaS 场景成立；自托管集群通常每个模型 1–3 个部署，机制价值（#3）已单独吸收，marketplace 本身不适用 |

## agent / MCP 方向（第二轮调研，2026-08-03）

覆盖 agentgateway（Solo.io → Linux Foundation，Rust，v1.4.1 / 2026-07-29；
Istio 1.30 已接入为实验性 GatewayClass）、Higress（16 个 AI 插件 + MCP 市场）、
Docker MCP Gateway、Cloudflare MCP Portals、IBM ContextForge、
AWS Bedrock AgentCore Gateway、MCP 供应链安全工具、A2A、Gateway API Inference Extension。

| # | 能力 | 来源 | 三家为何没有 / 现状 | 权重 | 落点 |
|---|------|------|-------------------|------|------|
| 10 | MCP 聚合/多路复用（并发扇出 list、前缀重命名、去重合并、每 target fail-open/close、stateless 模式合成 initialize） | agentgateway；Higress / Cloudflare / IBM ContextForge / AgentCore 各有形态 | 全行业都在做，**是本轮最强的"网关定义性"信号**。Edgion 原判断"明确不做"已于 2026-08-03 **翻案为要做** | 重 | AI-6 §1 |
| 11 | Tool 级授权 + 列表过滤（拒绝项从 `tools/list` 剔除） | agentgateway（CEL）、Higress（`allowTools` ∩ 请求头动态收窄）、Cloudflare（deny-by-default 允许列表） | 三家已收敛，属 table stakes；差异在"过滤列表"而不只是"拦调用" | 中 | AI-6 §2 |
| 12 | 委托式按用户身份到上游 MCP（RFC 8693 token exchange，subject token 缺失 fail-close；可选 Cross App Access / ID-JAG） | agentgateway | **本轮差异化最高**：Envoy AI GW 明确列为 roadmap（issue #2036），今天只能用静态 API key，摧毁按用户可审计性 | 中 | AI-6 §3 |
| 13 | Tool 面完整性固定 + 参数级 DLP（`tools/list` 规范化哈希锁定、运行时漂移检测防 rug pull；tool 参数/返回值扫密钥与 PII） | `mcp-warden`、Lasso、Docker `--block-secrets`、Cloudflare DLP | **没有任何主流网关内置**，只存在于点状产品 | 轻到中 | AI-6 §4 |
| 14 | Per-JSON-RPC-method 可观测的业界字段名（`mcp_requests_total{server,method,resource,resource_type}` + `mcp.*` 日志字段） | agentgateway | 方向 Edgion 已有（AI-6/03），此处是照抄已收敛命名以免自造 | 轻 | AI-6 §5 |
| 15 | GIE 成本判断下调（GIE 已 GA 且收窄成薄契约：只剩 InferencePool + LWEPP，调度器迁到 llm-d；agentgateway 证明非 Envoy 的 Rust 代理可一致性通过） | Gateway API Inference Extension / llm-d | 不是"新能力"，是**旧判断的修正**——路径 A 比原估计便宜 | 中偏重 | AI-7（判断更新） |

### 待评估（agent 方向）

| 能力 | 来源 | 为什么先不做 |
|------|------|-------------|
| REST/OpenAPI → MCP 合成（模板化请求/响应映射） | agentgateway、Higress（`openapi-to-mcpserver` CLI）、Kong | 价值明确但三家已覆盖，差异化低；且把 API 编排引入网关，边界外扩 |
| 语义 tool 搜索（把 tool 描述向量化，agent 用自然语言查，只加载相关的几个） | AWS Bedrock AgentCore Gateway | 需要 embedding + 向量索引进数据面 → 依赖 AI-5；但思路对"工具爆炸"是正解，值得在 AI-5 一并评估 |
| MCP Tasks（2026-07-28 规范的异步任务：`CreateTaskResult` + `tasks/get` 轮询） | agentgateway（目前唯一实现） | 大概率一年内成为必需，但现在只有一家实现，等规范落定 |
| A2A agent card 重写与注册 | agentgateway、Kong 3.14 | A2A 已 v1.0 稳定、150+ 组织，但**网关层流量远低于 MCP**，且治理只到身份+审计层 |

### 明确否决（agent 方向）

| 能力 | 来源 | 否决理由 |
|------|------|---------|
| Code Mode（把 N 个 OpenAPI 工具塌缩成单个 `run_code` 工具，在沙箱里跑 agent 写的 JS） | agentgateway | 上限最高的反上下文膨胀机制，但要在数据面内嵌 JS 运行时与沙箱，成本与风险都过大；与"精简数据面"定位冲突。**理念可借鉴**（减少进入模型上下文的中间结果），实现不抄 |
| 容器沙箱化运行本地 MCP server | Docker MCP Gateway | 那是本地开发工具的形态，不是 K8s 数据面网关的职责 |

### 两条架构警示（针对 Edgion 的 Pingora 形态）

1. **MCP 检查强制完整请求体缓冲。** #11 及以上的所有能力都要解析完整 JSON-RPC 请求。
   这与既有的请求级 `BodyStorage` 兼容，但意味着 MCP 路由必须显式开启 body 捕获并带硬上限，
   超大 tool-call 载荷需要明确的拒绝路径。
2. **聚合的 list-merge 与"响应恒流式"不变量的关系必须摆正。** 合并 N 个上游的
   `tools/list` 需要聚合 N 份完整响应——这应归入 `AGENTS.md` body 不变量**第 3 条**
   （插件发起的出站调用是独立的有界交换：硬字节上限、硬超时、有界并发），
   合并结果作为本地响应写出。**不得**为此放宽"主上游响应恒为流式"这条 🔴 规则；
   `tools/call` 结果与 SSE 通知流必须留在流式路径上。

## 来源

LiteLLM：https://docs.litellm.ai/docs/routing 、 https://docs.litellm.ai/docs/proxy/users 、
https://docs.litellm.ai/docs/proxy/cost_tracking 、 https://docs.litellm.ai/docs/simple_proxy
· Portkey：https://portkey.ai/docs/product/ai-gateway
· OpenRouter：https://openrouter.ai/docs/guides/routing/provider-selection 、
https://openrouter.ai/blog/insights/model-routing/
· Cloudflare：https://developers.cloudflare.com/ai-gateway/features/dynamic-routing/ 、
https://developers.cloudflare.com/ai-gateway/observability/logging/
· APISIX：https://apisix.apache.org/docs/apisix/plugins/ai-rate-limiting/ 、
https://apisix.apache.org/docs/apisix/next/plugins/ai-proxy-multi/
· archgw/Plano：https://github.com/katanemo/archgw
· Azure APIM：https://learn.microsoft.com/en-us/azure/api-management/llm-token-limit-policy 、
https://learn.microsoft.com/en-us/azure/api-management/llm-content-safety-policy
· AWS Bedrock Guardrails：https://aws.amazon.com/blogs/machine-learning/build-responsible-ai-applications-with-amazon-bedrock-guardrails/
· Helicone：https://www.helicone.ai/blog/how-to-gateway 、 https://www.mintlify.com/blog/mintlify-acquires-helicone
· TrueFoundry：https://www.truefoundry.com/ai-gateway
· Wealthsimple：https://engineering.wealthsimple.com/get-to-know-our-llm-gateway-and-how-it-provides-a-secure-and-reliable-space-to-use-generative-ai
· 三家基线：https://aigateway.envoyproxy.io/release-notes/v1.0/ 、
https://developer.konghq.com/ai-gateway/ 、
https://www.helpnetsecurity.com/2026/03/17/traefik-labs-triple-gate-new-capabilities/
