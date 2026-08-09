# AI-6 - Agent / MCP 治理

**依赖**: AI-0（流式地基）
**新依赖**: 纯 Rust
**在路线图中的位置**: **独立支线**。它挂在 AI-0 之后，但不解锁任何 LLM 能力，
也不被 AI-1..AI-5 依赖。可以随时插队做，也可以一直不做。
**来源**: 原仓库根目录 `mcp/` 六份任务，其中 01 与 03 的通用部分已上移到 AI-0

## 背景

MCP（Model Context Protocol）**不是 SSE 协议**。它的报文格式是 JSON-RPC 2.0，
SSE 只是它可选的一种响应编码。理清这一点决定了网关要做什么：

| 传输 | 形态 | 网关是否关心 |
|------|------|-------------|
| stdio | 本地进程管道 | 否 |
| **Streamable HTTP**（当前规范，2025-03-26 起） | 单端点（如 `/mcp`）；POST 发 JSON-RPC，响应体是 `application/json` **或** `text/event-stream`；可选 GET 开服务端推流 | **是** |
| HTTP+SSE（2024-11-05 旧规范，已废弃） | 双端点：`GET /sse` 拿流 + `POST /messages` 发消息 | 存量兼容 |

关键协议头：

| Header | 作用 |
|--------|------|
| `Mcp-Session-Id` | `initialize` 响应下发，客户端后续每请求携带。**MCP server 是有状态的** |
| `MCP-Protocol-Version` | 版本协商 |
| `Last-Event-ID` | SSE 断线重连续传 |
| `Accept: application/json, text/event-stream` | 客户端声明可接收两种响应形态 |

## 子任务索引

| 文件 | 优先级 | 内容 | 预估 |
|------|--------|------|------|
| [01-session-routing.md](01-session-routing.md) | **P0** | 会话路由：一致性哈希模式 + session-id 编码模式 | 中 |
| [02-oauth-resource-server.md](02-oauth-resource-server.md) | P1 | OAuth 2.1 Protected Resource Metadata + Bearer 校验 | 大 |
| [03-jsonrpc-governance.md](03-jsonrpc-governance.md) | P2 | method 级指标、session/tool 级限流（需解析 body） | 大 |
| [04-security-and-compat.md](04-security-and-compat.md) | P2 | Origin 校验、CORS、协议版本协商、旧版 HTTP+SSE 兼容 | 中 |
| [05-retry-semantics.md](05-retry-semantics.md) | P1 | MCP 特有的重试语义与亲和/failover 交互 | 小 |

已上移到 AI-0 的部分（本目录不再重复，只引用）：

- SSE / 流式响应透明直通（原 `mcp/01` 全文）→ [../00-streaming-foundation.md](../00-streaming-foundation.md)
- 请求体缓存与 body replay、首字节后禁止 failover（原 `mcp/03` 的通用部分）→ 同上

## 落地顺序建议

1. **AI-0 先做完**。做完之后网关就能正确代理 MCP、OpenAI 流式、A2A 等全部流式流量，
   这是投入产出比最高的部分，且不属于本支线
2. **01 + 04 的 Origin/CORS 部分**：01 几乎是白捡（能力已具备，只差空 key 语义和测试）；
   CORS 里 `Access-Control-Expose-Headers: Mcp-Session-Id` 缺了会让浏览器端 MCP client
   完全不可用，属于"不做就是坏的"
3. **02 + 05**：生产可用性。02 做完后端 MCP server 不用各自实现 OAuth
4. **03**：治理，需要真正解析协议，成本明显更高，按需启动

## 决策变更：多 MCP server 聚合从"不做"改为"要做"（2026-08-03）

**旧决策（已作废）**：不做多 MCP server 聚合，理由是"会把网关变成有状态的 MCP 协议实现，
与 Edgion 透明代理 + 插件的形态相距很远"。

**新决策**：**要做**。翻案依据有二：

1. **它不是某一家的特色，而是网关定义性能力。** agentgateway、Higress、
   Cloudflare MCP Portals、IBM ContextForge、AWS Bedrock AgentCore 全都提供，
   形态高度一致（单端点、多上游、tool 名加前缀、列表合并）。"一个 agent 同时接多个
   MCP server"就是 MCP 网关的主场景，不是边缘需求。
2. **原顾虑有解，实现不必有状态。** 见下方第 1 节：扇出合并属于"插件发起的有界出站
   交换"（`AGENTS.md` body 不变量第 3 条），合并结果作为本地响应写出；
   `tools/call` 与 SSE 通知流仍走流式路径。网关不需要成为有状态的 MCP 实现，
   也**不需要**放宽"主上游响应恒为流式"这条 🔴 规则。

优先级：与 tool 级授权（第 2 节）同级，都属 MCP 网关的核心能力面。
实现顺序仍在 01/04/02 这些基础项之后——聚合依赖会话路由与协议解析先就位。

注意区分：**session-id 编码**（[01-session-routing.md](01-session-routing.md) 的模式 B）
是独立的技术手段，可脱离聚合单独使用，也可作为聚合的组成部分。

## 竞品能力吸收（2026-08-03 调研）

来源与总表见 [../COMPETITIVE-ABSORPTION.md](../COMPETITIVE-ABSORPTION.md)。
本轮调研覆盖 agentgateway（Solo.io → Linux Foundation，Rust 实现，v1.4.1）、
Higress（16 个 AI 插件 + MCP 市场）、Docker MCP Gateway、Cloudflare MCP Portals、
IBM ContextForge、AWS Bedrock AgentCore Gateway 与一批 MCP 供应链安全工具。

### 1. MCP 聚合/多路复用（已决定要做，见上方决策变更）

agentgateway 的做法值得细看，因为它回答了当初"会变成有状态实现"的顾虑：

- 一个 backend 列多个 target 时，`tools/list` / `prompts/list` / `resources/list`
  **并发扇出**到所有上游，各自过 RBAC 过滤后按 `{target}_{name}` 重命名、去重、合并；
  调用时从前缀解析回目标
- 每个 target 有显式的 **FailClosed（默认）/ FailOpen** 失败策略——一个慢的或死的
  上游不能让整个 `tools/list` 挂掉
- `statefulMode: stateless` 时网关**为每请求合成** `initialize` + `initialized`
  通知，从而对接本身无服务端会话状态的上游

与 Edgion 现有不变量的关系（重要）：**扇出合并属于 `AGENTS.md` HTTP body 不变量第 3 条
"插件发起的出站调用是独立的有界交换"**，不是主上游响应路径。合并结果作为本地响应写出，
`tools/call` 的结果与 SSE 通知流仍然走流式路径。**不得**为了聚合而放宽"主上游响应恒为
流式"这条 🔴 规则。这正是原决策担心的问题的解法。

- [ ] 后端资源模型：一个 MCP backend 可列多个 target（与 AI-1a 的 `EdgionAIService`
      是并列关系，不要复用同一资源——一个是 LLM provider，一个是 MCP server 集合）
- [ ] `tools/list` / `prompts/list` / `resources/list` 并发扇出 + 合并；
      并发子调用必须有**全局并发上限 + 每次调用硬字节上限 + 硬超时**（body 不变量第 3 条）
- [ ] 命名前缀策略 `{target}_{name}` 与冲突处理；URI 类资源（如 `memo://insights`
      → `a+memo://insights`）的重写规则
- [ ] 每 target 的 fail-open / fail-close，**默认 fail-close**；一个上游超时或返回错误
      不能让整个 `tools/list` 挂掉，但也不能静默少返回工具（fail-open 时必须在响应
      元数据或日志中体现缺失）
- [ ] 调用路由：从 tool 名前缀解析回目标 target，前缀不匹配时的错误语义
- [ ] `statefulMode: stateless`——上游无服务端会话状态时，网关为每请求合成
      `initialize` + `initialized`；与 01 号任务的会话亲和是互斥的两种模式，需明确
- [ ] 与第 2 节 tool 级授权的交互：**先 RBAC 过滤再合并**，被拒条目不进入合并结果

### 2. Tool 级授权 + 列表过滤（权重：中，建议 P1）

**这是被引用最多的 MCP 治理需求**，三家与 agentgateway 已经收敛到同一形态，属于table stakes：
对 tool / prompt / resource 名、target、JWT claim 写表达式策略，**被拒的条目要从
`tools/list` 响应里剔除**——过滤"列表"而不只是拦"调用"，才能真正减少 prompt 层攻击面
（模型看不到的工具不会被诱导去调）。

- [ ] 策略表达式复用 Edgion 既有的 plugin conditions / DSL，**不要引入第三套表达式引擎**
- [ ] 列表过滤需要解析并改写响应体 → 依赖 AI-0 的可写管线；注意 `tools/list` 是
      非流式 JSON 响应，属有界改写
- [ ] Higress 的补充点：静态 `allowTools` 与请求头 `x-envoy-allow-mcp-tools` 取交集，
      让上游 authz 服务能按调用方动态收窄工具面而不必改网关配置——这是很干净的多租户缝

### 3. 委托式的按用户身份到上游（权重：中，**差异化最高**）

调研里最明确的**未被满足**的需求：Envoy AI Gateway 把 RFC 8693 token exchange 列在
roadmap，今天仍只能用静态 API key（`MCPBackendSecurityPolicy` 存 Secret），
这会**摧毁按用户的可审计性**——上游只看到一个共享服务账号。

- [ ] RFC 8693 token exchange：用下游用户身份换取上游 MCP server 的 token
- [ ] subject token 缺失时**必须 fail-close**（agentgateway v1.4.1 专门修了这点）
- [ ] 可选：Cross App Access / ID-JAG（Okta 主导，已成为 MCP 授权的官方扩展）
- [ ] 出站换 token 的调用走 `build_outbound_client`；token 需缓存与刷新

### 4. Tool 面完整性固定 + 参数级 DLP（权重：轻到中，**无人做**）

目前只存在于点状产品（`mcp-warden`、Lasso、Docker 的 `--block-secrets`、
Cloudflare 的 DLP 路径），**没有任何主流网关内置**：

- [ ] 把批准过的 `tools/list` 规范化后取 SHA-256 存为锁定值，运行时重新指纹比对，
      检测 **rug pull**（工具描述被悄悄篡改）与 tool poisoning
- [ ] 对已解析的 tool 调用**参数与返回值**扫描密钥/PII，命中可 block 或脱敏
      （复用 AI-4 的 PII 能力，作用域换成 tool 参数而非 prompt 文本）
- [ ] 漂移检测命中时的动作要可配置（告警 / 拒绝），默认告警——避免上游正常升级导致全面拒服

### 5. Per-method 可观测（权重：轻，**性价比最高**）

`03-jsonrpc-governance.md` 已覆盖方向，这里补上业界已收敛的字段名，直接照抄可省去自造：

- [ ] 指标 `mcp_requests_total{server, method, resource, resource_type}`
- [ ] 日志字段 `mcp.method.name`、`mcp.session.id`、`mcp.target`、
      `mcp.resource.type`、`mcp.resource.uri`、`gen_ai.tool.name`
- [ ] 注意基数：`resource.uri` 是高基数，按 `cicd/checks/check_metrics_inventory.py`
      的守卫规则它只能进日志，**不能进指标标签**

### 6. OAuth resource server 的具体规格（补充 02 号任务）

`02-oauth-resource-server.md` 的方向正确，补两个实现细节：需要同时提供
`/.well-known/oauth-protected-resource`（RFC 9728）与
`/.well-known/oauth-authorization-server`，且失败时的 `WWW-Authenticate` 挑战里要带
metadata URL——客户端靠这个自动发现。JWT 校验支持 strict / optional / permissive 三档。

### 7. A2A：先做便宜的 80%（权重：轻）

A2A 已于 2025-06 交给 Linux Foundation，2026-04 满一年时 150+ 组织、v1.0 稳定、
Agent Card 支持签名。但**网关层治理只有 agentgateway 与 Kong（3.14，2026-04）两家**，
且做的主要是身份 + 审计 + 内容检查，不是深度协议语义。**A2A 流量远低于 MCP**。

- [ ] 结论：不值得优先做。便宜的 80% 是 **agent card 的发现与 URL 重写**
      （把 card 里的地址改写成指向网关），其余等需求出现
- [ ] 若做，identity + audit 优先于协议语义

## 待补充的子任务文件

上述第 1、3、4 节内容较多，若决定启动应各自拆成独立文件（`06-*.md`），
本 README 只做索引。第 2、5、6、7 节已并入既有子任务的范围。
