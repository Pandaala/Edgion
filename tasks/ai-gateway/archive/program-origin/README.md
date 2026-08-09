# AI Gateway 能力路线图

创建时间：2026-07-29
Profile：`investigation`（本目录是程序级路线图；每个子项目成熟后各自转为独立 `feature` 任务）

## 目标

让 Edgion 具备可对外宣称的完整 AI Gateway 能力矩阵。对标 Envoy AI Gateway、
Kong AI Gateway、Traefik Hub AI Gateway 三家的并集，去重后按**依赖顺序**（不是价值顺序）拆分。

## 依赖图

```
AI-0  流式与请求体地基            ← 所有 AI 能力的物理前提
  ├── AI-1  LLM Provider 抽象层      ← "AI Gateway" 这个词的核心
  │     ├── AI-2  计量与成本治理
  │     ├── AI-3  韧性与路由策略
  │     ├── AI-4  安全护栏 pipeline
  │     │     └── AI-5  语义层（需解锁向量依赖）
  │     └── AI-7  推理感知路由（自托管场景，可选）
  └── AI-6  Agent / MCP 治理          ← 独立支线，不解锁任何 LLM 能力
```

## 子项目索引

| 编号 | 文件 | 依赖 | 新依赖 | 对标 |
|------|------|------|--------|------|
| AI-0 | [00-streaming-foundation.md](00-streaming-foundation.md) | 无 | 纯 Rust | 三家均视为基建 |
| AI-0a | resolved — answered by pingora fork commits `b1d520e` + `b829a34` (canonical request-body transport interface; see AI-0 status note) | AI-0 design review | 无（仅调研） | Pingora transport boundary |
| AI-1 | [01-provider-abstraction.md](01-provider-abstraction.md) | AI-0 | 纯 Rust | Envoy AIServiceBackend / Kong ai-proxy / Traefik 集中凭证 |
| AI-2 | [02-metering-and-cost.md](02-metering-and-cost.md) | AI-0、AI-1 | 纯 Rust | Kong ai-rate-limiting-advanced / Traefik token 级成本控制 |
| AI-3 | [03-resilience-and-routing.md](03-resilience-and-routing.md) | AI-1 | 纯 Rust | Kong 6 种 LLM LB / Traefik 多 provider 韧性 |
| AI-4 | [04-guardrails.md](04-guardrails.md) | AI-0、AI-1 | 纯 Rust + 外部 HTTP | Traefik composable safety pipeline / Kong prompt guard |
| AI-5 | [05-semantic-layer.md](05-semantic-layer.md) | AI-4 | **分词器 + 向量** | Kong 3.8+ 语义系列 / Traefik proactive token estimation |
| AI-6 | [06-mcp-and-agent/](06-mcp-and-agent/README.md) | AI-0 | 纯 Rust | Envoy MCP Gateway / Traefik MCP Gateway |
| AI-7 | [07-inference-aware-routing.md](07-inference-aware-routing.md) | AI-1 | 需 vLLM 指标对接 | Gateway API Inference Extension |

## 竞品能力吸收

对标范围不止三家。2026-08-03 对 LiteLLM / Portkey / OpenRouter / Cloudflare AI Gateway /
APISIX / archgw / Azure APIM / Bedrock Guardrails / Helicone / TrueFoundry 等做了调研，
结论（已吸收 9 项、待评估 3 项、明确否决 2 项，含理由与来源）在
[COMPETITIVE-ABSORPTION.md](COMPETITIVE-ABSORPTION.md)，具体需求已写入 AI-2 / AI-3 / AI-4
各自的"竞品能力吸收"小节。agent/MCP 方向的第二轮调研待补入 AI-6 / AI-7。

## 依赖解锁策略

已决策（2026-07-29）：**按阶段解锁**。第一阶段只用纯 Rust；需要分词器（tiktoken /
HF tokenizers）或向量能力（embedding 调用 + Redis 向量索引）的能力集中在 AI-5，
到时再单独决策是否引入。在此之前：

- token 计量只用 provider 返回的 `usage` 字段（事后准确，无法事前拦截）
- 不做语义缓存、语义路由、语义 prompt guard
- 护栏只做 regex / 关键词 / PII 正则，不做语义判定

## 落地顺序

**第一阶段 = AI-0 全部 + AI-1a（单 provider 打通）**，验收标准：

> 客户端用标准 OpenAI SDK 把 `base_url` 指向 Edgion、**本地不配任何 API key**，
> 流式对话逐字吐出，网关侧出 token 用量指标。

理由：地基一次做对；第一个里程碑即可演示；不引新依赖；schema 转换（AI-1b）接上时
流式框架已就位，不返工。

被否决的两条路径，记录理由以免反复：

- **价值先行**（第一阶段直接做完整 AI-1 含三向 schema 转换）：跨 provider 的流式
  chunk 逐块翻译在没有 AI-0 可写管线时会写得极扭曲，返工概率高。
- **治理先行**（第一阶段做 AI-0 + AI-2，不碰 provider 抽象）：透传模式下各 provider
  的 `usage` 字段格式不同，仍需写一层解析，等于偷偷做了半个 AI-1，两头不靠。

## 与原 `mcp/` 目录的关系

原仓库根目录的 `mcp/`（2026-07-27，六份任务 + README）已整体并入本目录。映射如下：

| 原文件 | 去向 | 说明 |
|--------|------|------|
| `mcp/01-sse-streaming-passthrough.md` | **AI-0** | 内容全部上移。原文只覆盖"别把流式弄坏"的消极面，AI-0 补上"能在流式上做事"的积极面 |
| `mcp/03-retry-and-failover-boundary.md` | **AI-0 + AI-3 + AI-6/05** | 拆三份：body replay 与首字节后禁 failover 是通用地基（AI-0）；跨 provider fallback 归 AI-3；MCP 亲和与 failover 的交互留在 AI-6 |
| `mcp/02-session-affinity.md` | AI-6/[01-session-routing.md](06-mcp-and-agent/01-session-routing.md) | 补充了 session-id 编码模式作为第二种路由策略 |
| `mcp/04-mcp-oauth-resource-server.md` | AI-6/[02-oauth-resource-server.md](06-mcp-and-agent/02-oauth-resource-server.md) | 基本原样 |
| `mcp/05-jsonrpc-observability-ratelimit.md` | AI-6/[03-jsonrpc-governance.md](06-mcp-and-agent/03-jsonrpc-governance.md) | 标注限流核心与 AI-2 共用 |
| `mcp/06-security-and-compat.md` | AI-6/[04-security-and-compat.md](06-mcp-and-agent/04-security-and-compat.md) | 原样 |

六份里只有一份半落在 AI 主线上，其余四份半整体构成 AI-6 支线。

## 现状盘点（2026-07-29 扫描）

Edgion 现有的、可直接复用的原料：

| 资产 | 路径 | 对 AI Gateway 的用途 |
|------|------|---------------------|
| 动态上游选择插件 | `edgion-gateway/src/plugins/http/dynamic_external_upstream/`、`dynamic_internal_upstream/` | model 名 → backend 路由的现成模式 |
| 请求体缓存 | `edgion-gateway/src/request_body/`（`storage.rs` 支持内存→磁盘溢出） | 拿完整 prompt；跨 provider 重试的 body replay |
| 请求体缓存插件 | `edgion-gateway/src/plugins/http/request_body_buffer/` | 按路由开启完整 body 的既有开关 |
| Redis 限流核心 | `edgion-gateway/src/plugins/common/redis_rate_limiter`、`plugins/http/rate_limit_redis/` | token 限流：把成本从固定 1 改成动态 token 数 |
| Secret 存储 | `edgion-common/src/secret/secret_store.rs` | provider 凭证托管 |
| 一致性哈希（按 header 取 key） | `edgion-resources/src/resources/edgion_backend_traffic_policy.rs:327` → `edgion-gateway/src/backends/selector.rs:150` | MCP 会话亲和；LLM 亲和路由 |
| 响应头钩子 | `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs` | 流式响应识别的置位点 |
| 响应体逐块钩子 | `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs` | 流式处理的挂载点（**当前只读，见 AI-0**） |
| 外部扩展逃生口 | `plugins/http/ext_proc/`、`plugins/http/wasm/`、EdgionDSL | 自定义 guard / 自定义计费逻辑 |

已确认的空白（无任何实现）：`text/event-stream` 专门处理、JSON-RPC 解析、任何 LLM
provider 相关代码。

## Cross-cutting constraints (inherited from the superseded 2026-07-18 roadmap)

These bind every AI subproject, not just phase 1:

- No plugin may independently buffer and parse the same LLM body; every inspected
  body has a configured byte limit and a deterministic overflow policy.
- Sensitive prompts, completions, credentials, and PII mappings never enter
  PluginLog, tracing, or ordinary access logs.
- External calls are forbidden from the synchronous response-body chunk hook.
- Response policies must declare whether they preserve streaming, buffer, or
  reject streaming requests.
- Retries stop after any response bytes have been committed downstream (now
  enforced by the fork).
- Provider retries and token reconciliation are idempotent per logical request.
- Plugin configuration is statically validated in `edgion-resources` before
  Gateway runtime construction.
- User documentation: when a feature ships, add a dedicated AI-gateway section
  under `docs/en/user-guide/` (with the `docs/zh-CN/` mirror) instead of
  scattering pages.

## Status

**AI-0, AI-1a, and the Anthropic-first AI-1b slice are complete (2026-08-03).** The shipped work
includes `EdgionAIService`, the request-only `AiProxy` plugin, OpenAI/OpenAI-compatible passthrough,
native Anthropic Messages translation, centralized Secret credentials with hot rotation, H1/H2
incremental SSE, bounded reported/estimated token usage, durable per-request access-log fields, and
an optional low-cardinality token counter that is disabled by default. Bedrock/cloud-auth and
multi-service provider selection remain deferred AI-1 work; AI-2 is the next execution stage.

The approved phase design is
`docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md`. AI-0 request-side transport was
integrated through the Edgion Pingora fork before this phase; all AI-0/AI-1a response-side work and
the first AI-1b translation slice were implemented in Edgion without an additional fork change.

This directory was imported from the ws5 workspace on 2026-08-03. It supersedes
`tasks/todo/llm-ai-gateway/`, `tasks/ai-gateway/`, and the five former
`tasks/todo/llm-*.md` files; their applicable content is consolidated here and in
`tasks/todo/response-body-buffer-runtime.md`.
