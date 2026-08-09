# AI Gateway 路线图 — Agent 执行手册（操作者专用，中文）

> 给**操作者（你）**用的手册。每一步一个标题，标题下的代码块是**完整、自足的
> 启动内容**——直接整块复制给一个新 agent 即可，不需要再拼接任何其它段落。
> 仓库"落盘内容英文-only"规则对本文件例外（操作手册，按用户要求中文书写）。
>
> 每步开一个**全新 agent**（不要复用上一步的会话，上下文污染会导致 scope 蔓延）。

## 当前进度（每完成一步就更新这一行）

- ✅ 规划完成：设计梳理、agentgateway 对比、竞品吸收、7 个工作流路线图、5 个
  决策点全部关闭、ws4 工作区文档已吸收。
- ✅ 第 2 步 WS-00 合流**已完成**（2026-08-09）：本工作区用
  `git merge origin/feature-06-24` 合入流中配额终止特性，合并提交 `2b6e5b23`
  已推送；ws2 那条 cherry-pick 线已废弃，其 PPv2 改动与本套规划文档已搬入本
  工作区。**本工作区是唯一主线。**
- ⏳ **下一步 = 第 3 步（WS-04 A2A 设计展开）**。
- ⬜ 之后依次：第 4 步 WS-04 实施 → 第 5-6 步 WS-06 SSE v2 → 第 7-10 步 WS-01
  MCP → 第 11-12 步 WS-02 入口广度 → 第 13-14 步 WS-03 护栏 → 第 15 步 WS-05
  模型路由 → 第 16 步 WS-07 收尾。

---

## 第 1 步：基线自检

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关，Controller/Gateway
分离；当前分支 feature-06-24）。

任务：基线自检，不修改任何文件。依次运行并贴出原始输出：
1. cargo check --workspace --all-targets
2. cargo clippy --workspace --all-targets
3. cargo fmt --all -- --check
4. python3 cicd/checks/validate_agent_docs.py
5. python3 cicd/checks/check_ssa_force.py
6. python3 cicd/checks/check_metrics_inventory.py
7. python3 cicd/checks/check_pingora_source_policy.py

如有报错，逐条列出并判断是否与 AI Gateway 工作相关。不要顺手修复无关问题，
只汇报。注意：第 7 项在 Cargo.toml 仍保留本地 pingora [patch] 时会失败，这是
已知且当前故意保持的状态（本地 fork 有在制品），如失败原因是这个就标注为预期。

最后给出结论：基线是否干净、能否开工。不要执行 git commit / git push。
```

**验收**：除已知的 pingora-source-policy 预期失败外全绿。

---

## 第 2 步：WS-00 — 双工作区合流（已完成，留档）

> **已于 2026-08-09 在本工作区（ws4）完成，无需再执行。** 做法：
> `git merge origin/feature-06-24` 把远端那两条（`b0c9db66` 流中配额终止 +
> `2bd656f9`）并入本地的 33 条 review 修复，产生合并提交 `2b6e5b23` 并已推送，
> 远端 `feature-06-24` 现在包含两股工作的全部内容。
>
> 冲突解法的完整对照表留在 `analysis/04-ws4-reconciliation.md` §3b（`usage.rs`
> 两边都保留、ws4 的新测试需改成 async 并补构造函数参数、review 台账不可整份
> 覆盖否则丢 D1/D2/D3、`test_server.rs` 各取一段）——将来再遇到同类合并可直接参考。
>
> 另一侧的 ws2 工作区（/Users/caohao/ws2/Edgion）曾用 cherry-pick 走过一遍同样的
> 合流，那条线已废弃；其 PPv2 改动与本套规划文档已搬入本工作区。**本工作区
> （ws4）是唯一主线**，后续所有工作在这里进行。

## 第 3 步：WS-04 A2A — 设计展开

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关，Controller/Gateway
分离；分支 feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork
（Pandaala/pingora 的 edgion 分支），Cargo.toml 直接依赖远端 fork。

【先读这三份，按序】
1. tasks/ai-gateway/README.md（总索引与状态）
2. tasks/ai-gateway/analysis/01-data-plane-design-review.md（现有数据面设计的
   可复核深度梳理：Pingora fork 的请求体捕获/重放 seam、响应体异步 filter +
   有界注入 + 终止、BodyStorage、SSE 解码器、src/ai 模块、拷贝次数表——所有论断
   带 file:line）
3. tasks/ai-gateway/roadmap/04-a2a-gateway.md（本任务的**范围权威**，架构决策
   已冻结，不要重新设计）
需要时再查：analysis/02-agentgateway-comparison.md（对标 agentgateway 的功能与
效率差距）、analysis/03-competitive-absorption.md（15 家竞品的吸收/拒绝决策记录，
提新想法前先查，避免重开已拒绝的方向）。

【已关闭的决策，禁止重开】①Edgion 是流量层网关，不做进程执行类功能；②MCP 身份/
RBAC/OAuth 整体延后到未来的授权系统课题；③body 不变量 2 不修订（有界 hold-and-
release 本就合法）；④Provider 只做 GPT/Claude/Gemini；⑤不做对照性能测试，质量靠
自身集成测试。

【任务】把 WS-04（A2A gateway）从 scoping 计划展开为可执行的 feature 任务包。
冻结的架构决策（来自 roadmap/04）：
- agent card 重写走"网关自发起的有界外呼"（build_outbound_client + 1 MiB 上限 +
  EgressResolver + 超时），以 PluginRunningResult::ErrTerminateRequest 返回本地
  响应；主代理绝不缓冲响应（这是 body 不变量 2 的要求）。
- 请求侧 JSON-RPC method 观测用 BodyStorage 的有界 JSON 扫描，不新增解析器。
- 支持三种卡片形态：/.well-known/agent.json 的 v0.2/v0.3 与
  /.well-known/agent-card.json 的 v0.3+（v1.0 是 supportedInterfaces[].url 数组）。

【必读补充】
- 插件开发指南：skills/01-architecture/02-gateway/12-edgion-plugin-dev.md
- 插件注册模式：edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs，
  并挑一个 request-stage 插件的 plugin_configs/*.rs 作为 schema 与校验的对照
- 有界外呼与出网统一规则：edgion-common/src/link_sys_core/http_client.rs 与
  skills/01-architecture/00-common/05-unification-points.md
- 请求体读取 API：edgion-gateway/src/plugins/runtime/traits/session.rs，以及
  analysis/01 文档的 §3.8 / §3.9
- 测试基建：skills/05-testing/01-integration-testing.md 与
  skills/05-testing/references/test-server-capabilities.md
- 任务文档规范：skills/07-tasks/SKILL.md（feature profile 的文件集）

【产出】在 tasks/ai-gateway/archive/completed-tasks/a2a-gateway/ 下建 feature 任务包（全英文）：
a2a-gateway.md（主文件）、00-context.md、01-design.md、02-implementation.md、
03-subtasks.md。01-design.md 必须含：三种卡片版本的字段级重写规则、externalUrl
推导（listener host/proto + X-Forwarded-Proto，可被配置覆盖）、
onCardFetchFailure 策略、method 观测字段清单、错误体形态（固定文案，细节进
access log）。02-implementation.md 要给到 TDD 步骤粒度的文件级改动清单，并为每个
对外行为写出集成测试场景（test_server 需新增 mock A2A 后端：三种卡片端点 +
JSON-RPC echo）。

【硬性规则】落盘内容一律英文；遵守 CLAUDE.md 的 body 不变量 1-5 与 Cross-cutting
规则 1-10；只写文档，不改代码；不要 git commit / git push。最后汇报设计要点与
开放问题。
```

**验收**：审 `01-design.md`（重点：卡片重写是不是外呼式、失败策略、配置面）。

---

## 第 4 步：WS-04 A2A — 实施

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【任务】按 tasks/ai-gateway/archive/completed-tasks/a2a-gateway/ 任务包实施 WS-04。设计已评审
通过，不要改设计；若发现设计问题，先停下来汇报。

【必读】
- 任务包全部文档，尤其 01-design.md 与 03-subtasks.md（按其顺序推进）
- 动代码前先读 skills/00-design-rules.md，提交前对照其 🔴 条目扫一遍 diff
- skills/01-architecture/02-gateway/12-edgion-plugin-dev.md（插件开发）
- skills/03-coding/00-logging-and-tracing-ids.md 与 01-log-safety.md（日志规范：
  数据面禁 tracing::warn!/error!/info!，走 access log + EdgionStatus）
- skills/05-testing/01-integration-testing.md（集成测试写法）

【执行要求】
- 严格 TDD：每个子任务先写失败测试、跑一遍确认失败、再写最小实现、再跑通过。
- 单测跟随代码文件放置（项目惯例）；每完成一个子任务更新 03-subtasks.md 状态。
- 新插件走完整注册链：EdgionPlugin 变体 → plugin_configs 校验 → CRD YAML；
  若 schema 结构化变化，跑 ./cicd/checks/check_crd_coverage.sh。
- test_server 的 mock A2A 后端按 02-implementation.md 实现；集成测试至少覆盖：
  三种卡片版本重写、X-Forwarded-Proto 推导、卡片拉取失败策略、method 观测字段。
- 完成后跑：cargo fmt --all、cargo check --workspace --all-targets、
  cargo clippy --workspace --all-targets、python3 cicd/checks/validate_agent_docs.py、
  以及本插件的集成单项。贴原始输出。

【硬性规则】落盘内容一律英文；遵守 CLAUDE.md 的 HTTP body 不变量（尤其"主代理
上游响应永远流式、不得新增响应侧缓存"）与 Cross-cutting 统一规则（外呼必须走
build_outbound_client + EgressResolver）；不要 git commit / git push，改完汇报
diff 概要等我指示。
```

**验收**：校验全绿 + 集成通过；建议再开一个 agent 跑 `/code-review`。

---

## 第 5 步：WS-06 SSE 解码器 v2 — 设计展开

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【先读】
1. tasks/ai-gateway/analysis/01-data-plane-design-review.md 的 §4.4-§4.6 与 §5.3
   （SSE 解码器现状、AI 用量提取与配额终止、逐事件拷贝账目——这是本次改造的基线）
2. tasks/ai-gateway/roadmap/06-sse-decoder-v2.md（**范围权威**）
3. tasks/ai-gateway/todo/sse-decoder-zero-copy-stream.md（既有设计讨论，其中
   F1-F3 事实与约束已验证，直接引用，不要重新论证）

【已关闭的决策，禁止重开】body 不变量 2 **不修订**——它禁的是"无界状态"，有界
hold-and-release 本就合法，且已有上线先例：Anthropic 翻译器在
edgion-gateway/src/ai/anthropic/response.rs:351 用 chunk.take() 跨 chunk 暂扣
未完成事件的字节。本工作流只在 CLAUDE.md 与 skills 加一条**权威澄清措辞**，条件是：
有界且 admission 校验、策略显式声明、hold-and-release、eos 必冲刷、默认关闭。
另：不做对照性能基准测试。

【任务】把 WS-06 展开为 feature 任务包，范围包含：
1. 切片链解码器：edgion-gateway/src/sse/ 重写为保留 Bytes 引用计数的增量解码器
   （行/事件表示为链上的 range，按需物化），保持现有 bounds/degrade/malformed
   语义与 1 MiB pending 上限，保持 feed_each 访问者形状以便消费者机械迁移。
2. 惰性 JSON：ai/usage.rs 热路径去掉逐事件 serde_json::Value，改为针对实际读取
   字段的定向提取；给翻译器留 Value 逃生口。
3. hold-and-release 窗口原语（sse/window.rs）：有界事件队列 + 文本窗口视图
   （按 token 估算大小、可配重叠），release() 返回原始 Bytes 切片或改写后的替代
   内容。供 WS-03 使用；窗口×terminate 的先后语义（mask 先放行、reject 丢弃）
   写进 trait 契约文档。
4. 事件管线：Anthropic 翻译器直接产出事件供用量提取器消费，消灭当前的二次解码
   （见 sse/mod.rs:28-30 的注释）。
5. 顺带解决 tasks/ai-gateway/todo/response-body-pipeline-hardening.md 的 D1b
   （malformed SSE 静默绕过配额 → 带容忍阈值的 fail-closed，阈值方案在设计里
   给选项让我拍板）与 D5（sealed Anthropic 流永不终止）。

【必读补充】
- 现状代码：edgion-gateway/src/sse/mod.rs（全文）、ai/usage.rs 的
  observe_chunk/on_chunk/终止门、ai/anthropic/response.rs 的 process_streaming、
  ai/dispatch.rs:427-455（处理器链组装）
- fork 响应侧契约：你本地 pingora fork checkout 下的 pingora-proxy/src/response_body_sink.rs
  与 proxy_trait.rs:584-596
- 任务文档规范：skills/07-tasks/SKILL.md

【产出】tasks/ai-gateway/archive/completed-tasks/sse-decoder-v2/ 任务包（全英文），01-design.md
必须含：chain.rs 的切片链数据结构、window.rs 的接口与契约、惰性提取方案、事件
管线改造、D1b 阈值选项、迁移策略（公开 API 名不变）、以及分阶段的验收方式。
验收无 benchmark 门槛，改为结构性标准：解码器核心不得有逐字节 push/push_str
路径，usage.rs 热路径不得有逐事件 serde_json::Value。

【硬性规则】落盘内容一律英文；只写文档不改代码；不要 git commit / git push。
```

**验收**：重点审窗口原语接口与 D1b 阈值方案（这是产品行为，要你拍板）。

---

## 第 6 步：WS-06 SSE 解码器 v2 — 实施

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【任务】按 tasks/ai-gateway/archive/completed-tasks/sse-decoder-v2/ 实施，严格按其 01-design.md
的阶段顺序，每个阶段独立可回归、逐阶段停下来汇报：
  阶段 1 解码器核心 + 现有单测 1:1 平移 —— 结束时必须**行为零变化**；
  阶段 2 usage.rs 惰性 JSON 提取；
  阶段 3 事件管线（翻译器单次解码，含 D5 修复）—— 结束时
    tasks/ai-gateway/archive/completed-tasks/ai-gateway-anthropic-translation/04-wire-contract.md
    所述的金样测试必须原样通过；
  阶段 4 窗口原语 + CLAUDE.md/skills 的"有界窗口"澄清措辞（措辞单独列给我过目
    后再写入）；
  阶段 5 D1b 门槛变更（按我拍板的方案）。

【必读】任务包全部文档；skills/00-design-rules.md（动代码前，提交前扫 🔴 条目）；
skills/03-coding/ 下的日志与可观测性规范；analysis/01 §4.4-§4.6 作为基线事实。

【硬性规则】
- 落盘内容一律英文。
- body 不变量：主代理上游响应永远流式；窗口必须有界、eos 必冲刷、默认关闭；
  绝不新增响应侧 BodyStorage。
- 新增 chain-boundary 属性测试（跨 chunk 字段切分、CRLF 跨 chunk、BOM、
  degrade/malformed 平价）。
- 全部既有 SSE/AI 集成场景必须绿，含 quota_termination_h2_framing。
- 校验命令：cargo fmt --all、cargo check --workspace --all-targets、
  cargo clippy --workspace --all-targets、cargo test -p edgion-gateway --lib ai::
  与 sse::。贴原始输出。
- 不要 git commit / git push。
```

**验收**：分阶段验收（阶段 1 的零行为变化、阶段 3 的金样必过）；澄清措辞你亲自过目。

---

## 第 7 步：WS-01 MCP — 设计展开

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关，Controller/Gateway
分离；分支 feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【先读】
1. tasks/ai-gateway/README.md
2. tasks/ai-gateway/analysis/01-data-plane-design-review.md（数据面现状，尤其
   §2 fork 的响应侧 sink 契约、§4 流式响应管线、§3.8 插件访问 API）
3. tasks/ai-gateway/roadmap/01-mcp-gateway.md（**范围权威**，含从 ws4 AI-6 吸收
   的加固清单）
背景参考：tasks/ai-gateway/archive/program-origin/06-mcp-and-agent/（原始 MCP 子计划，
其中 02-oauth-resource-server.md 与 README §2/§3 的授权部分**已被冻结、不实施**，
仅作未来授权系统课题的输入）。

【已关闭的决策，禁止重开】
- Edgion 是流量层网关：**stdio 上游传输永久不做**（进程执行属 sandbox/runtime
  职责，CRD 携带 cmd/args/env 等于把配置权变成代码执行权）；stdio 的 MCP server
  由用户用 sidecar / mcp-proxy 适配成 HTTP，文档要写清这个模式。
- **MCP 专属的身份/授权整体延后**（工具级 RBAC、OAuth Protected Resource
  Metadata、IDP 集成、token exchange 都不做）——Edgion 目前没有统一授权基座，
  为 MCP 单独做一套将来必然重写。路由级认证复用现有通用 auth 插件（jwt_auth、
  key_auth 等）。保留一个不涉身份的**静态 per-member tool 允许/屏蔽列表**。
- 不做对照性能测试。

【任务】把 WS-01 展开为 feature 任务包。冻结的架构：
- MCP 是**终结型服务**：路由 backendRef 用新 kind 短路进 MCP handler（参照
  edgion-gateway/src/plugins/http/direct_endpoint/ 与
  routes/http/proxy_http/pg_upstream_peer.rs 的短路模式）。客户端腿走正常下行
  会话（Streamable HTTP + 旧版 SSE）；**每次上游调用都是有界外呼**
  （build_outbound_client，1 MiB 上限，fail-not-truncate），上游通知流用会话
  持有的 SSE 连接增量解析（有界，绝不累积）——这样完全不需要响应侧缓存。
- 新 CRD EdgionMCPService（namespaced）描述一个上游 MCP server：transport
  (streamableHttp | sse)、endpoint、认证注入（secretRef）、超时、failure mode。
- 新插件 mcp_gateway（EdgionPlugin::McpGateway）在路由上配置联邦：成员列表、
  名称前缀模式、会话 TTL、静态 tool 允许/屏蔽列表。
- rmcp 库：kickoff 时先做 license/API 评估并记录结论——建议只借用协议类型定义，
  传输/会话/路由自研，以保证不变量与出网规则归我们掌控。
- 协议版本目标：baseline 2025-03-26 + 2025-06-18（追新是长期跟随项）。
- 从 ws4 吸收、必须进设计的加固项：Origin 头校验（DNS-rebinding，MCP 规范要求）、
  CORS 必须暴露 Mcp-Session-Id（否则浏览器端 MCP 客户端完全不可用）、
  MCP-Protocol-Version 协商、Last-Event-ID 透传、tools/list 哈希锁定（rug-pull
  检测）、策略拒绝一律 JSON-RPC 错误体（不是裸 429）、会话目标已死时快速失败并
  提示重新 initialize（绝不跨实例重试）、无状态成员的合成 initialize、
  per-member 失败模式默认 FailClosed。

【必读补充】
- 新资源接入清单：skills/01-architecture/01-controller/09-add-new-resource/00-guide.md
  与 skills/01-architecture/00-common/03-resource-system.md
- CRD 与热加载范本：edgion-resources/src/resources/edgion_ai_service.rs 与
  edgion-gateway/src/ai/service_store.rs（ArcSwap 索引）
- 有界外呼与出网统一规则：edgion-common/src/link_sys_core/http_client.rs、
  skills/01-architecture/00-common/05-unification-points.md
- SSE 解码器：edgion-gateway/src/sse/（若 WS-06 已完成，用新版接口）
- 任务文档规范：skills/07-tasks/SKILL.md

【产出】tasks/ai-gateway/archive/completed-tasks/mcp-gateway/ 任务包（全英文）。01-design.md 必须
含：EdgionMCPService 的 CRD schema、mcp_gateway 插件 schema、会话存储设计
（TTL + reaper + 后端固定 + 无状态包装）、联邦路由与 list 合并语义、通知 fan-in
的有界策略、错误映射表、以及上面那份加固清单的落点。03-subtasks.md 按 roadmap 的
三个 phase 切分。

【硬性规则】落盘内容一律英文；只写文档不改代码；不要 git commit / git push。
```

**验收**：审 CRD/插件 schema（操作面发布后难改）与会话设计；rmcp 取舍结论。

---

## 第 8 步：WS-01 MCP — Phase 1 实施（单上游代理）

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【任务】实施 tasks/ai-gateway/archive/completed-tasks/mcp-gateway/ 的 Phase 1（单上游代理），
按其 03-subtasks.md 顺序推进。设计已评审通过，不要改设计；发现问题先停下汇报。
范围：EdgionMCPService CRD + ArcSwap store → mcp_gateway 插件 + 路由终结分发 →
Streamable HTTP 客户端腿 + 有界上游交换 + 协议版本协商 → 会话存储
（TTL + reaper + 后端固定 + 无状态合成 initialize）。

【必读】任务包全部文档；skills/00-design-rules.md（动代码前，提交前扫 🔴 条目）；
skills/01-architecture/01-controller/09-add-new-resource/00-guide.md（新资源全流程）；
skills/03-coding/ 的日志规范；skills/05-testing/01-integration-testing.md。

【执行要求】
- 严格 TDD；单测跟随代码文件放置；每完成一个子任务更新 03-subtasks.md。
- test_server 增加 mock MCP server 模式（initialize / tools/list / tools/call +
  会话行为）。
- 集成场景至少：端到端 round trip、会话过期、上游超大响应 fail-closed。
- 校验：cargo fmt --all、cargo check --workspace --all-targets、
  cargo clippy --workspace --all-targets、./cicd/checks/check_crd_coverage.sh、
  python3 cicd/checks/validate_agent_docs.py，以及新增集成单项。贴原始输出。

【硬性规则】落盘内容一律英文；上游调用必须是有界外呼（build_outbound_client +
EgressResolver + 1 MiB 上限，禁止代理环境变量、禁止跟随重定向）；绝不新增响应侧
缓存；MCP 层本身不做身份判定（认证交给路由上的通用 auth 插件）；不要 git commit /
git push。
```

## 第 9 步：WS-01 MCP — Phase 2 实施（联邦聚合）

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【任务】实施 tasks/ai-gateway/archive/completed-tasks/mcp-gateway/ 的 Phase 2（联邦聚合）：
多成员路由（tools/prompts/resources 名称前缀、list 合并 fanout、per-member
failOpen/failClosed，默认 FailClosed）、静态 per-member tool 允许/屏蔽列表
（不涉身份，纯配置层过滤）、通知 fan-in（各成员 SSE 流 → 客户端流，用有界
SSE 解码器，溢出按解码器既有降级策略处理，绝不累积）。

【必读】任务包 01-design.md 的联邦章节与 03-subtasks.md；skills/00-design-rules.md；
edgion-gateway/src/sse/（解码器接口）；skills/05-testing/01-integration-testing.md。

【集成场景】双成员联邦；单成员宕机的两种失败模式对比；前缀冲突在控制器侧被校验
拒绝；过滤后的 tool 列表符合配置。

【硬性规则】落盘内容一律英文；严格 TDD；有界外呼规则同 Phase 1；校验命令同
Phase 1 并贴原始输出；不要 git commit / git push。
```

## 第 10 步：WS-01 MCP — Phase 3 实施（打磨）

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。

【任务】实施 tasks/ai-gateway/archive/completed-tasks/mcp-gateway/ 的 Phase 3（打磨）：
旧版 SSE 客户端腿；WS-03 护栏的挂载点预留（本步不实现具体护栏）；观测字段
（按项目克制策略：access log + EdgionStatus 为主，任何新 metric 必须先过
python3 cicd/checks/check_metrics_inventory.py 并同步
docs/en/ops-guide/observability/metrics-inventory.md）；用户文档
docs/en/mcp-gateway/ 与镜像 docs/zh-CN/mcp-gateway/，其中必须写清 stdio 类
MCP server 用 sidecar / mcp-proxy 适配的模式（我们永久不做进程执行）。

【必读】任务包文档；skills/03-coding/observability/ 下的指标与日志规范；
skills/07-tasks/SKILL.md 的任务收尾清单（文档落点表）。

【硬性规则】落盘内容一律英文（docs/zh-CN/ 是唯一例外，可写中文）；校验命令：
cargo fmt --all、cargo check --workspace --all-targets、clippy、
validate_agent_docs.py、check_metrics_inventory.py，贴原始输出；
不要 git commit / git push。
```

---

## 第 11 步：WS-02 LLM 入口广度 — 设计展开

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【先读】
1. tasks/ai-gateway/analysis/01-data-plane-design-review.md 的 §4.5-§4.6
   （src/ai 模块全景、翻译器与用量提取的现状）
2. tasks/ai-gateway/roadmap/02-llm-ingress-breadth.md（**范围权威**）
3. tasks/ai-gateway/archive/completed-tasks/ai-gateway-anthropic-translation/ 的 01-design.md 与
   04-wire-contract.md（**已上线的翻译器范本**：白名单纪律、状态/媒体类型矩阵、
   金样 wire contract 方法论——新方言与新 provider 都要有同等的 wire contract 文档）

【已关闭的决策，禁止重开】Provider 只做最流行三家：GPT（OpenAI，已有）、Claude
（Anthropic，已有）、Gemini（新增，用 API-key 的 Gemini API）。**不做 Bedrock**
（SigV4 + AWS 二进制 event-stream 不值当）；Azure 与 Vertex-OAuth 延后到有需求
再说。另：不做对照性能测试；body 不变量不修订。

【任务】把 WS-02 展开为 feature 任务包，三个 Phase：
- Phase 1：/v1/messages Anthropic 原生入口。请求侧 Anthropic Messages → 现有的
  canonical OpenAI 内部形态（镜像 ai/anthropic/request.rs 的白名单纪律，遇到
  不支持字段 fail closed）；响应侧当上游是 OpenAI 形状时把流翻译回 Anthropic
  SSE。**关键效率要求**：当客户端方言与 provider 方言相同（Anthropic→Anthropic、
  OpenAI→OpenAI）必须是**字节直通 + 旁路提数**，禁止翻译两跳、禁止重序列化——
  为此需要给 usage 提取器加一个"读 Anthropic 事件"的模式。
- Phase 2：/v1/models 合成（本地响应，不发上游）+ /v1/embeddings（非流式有界类，
  只做用量提取）。
- Phase 3：Gemini provider（AiProvider::Gemini）：API-key 凭证复用
  AiApiKeyResolver 槽位、generateContent/streamGenerateContent 的 URI 映射、
  请求/响应翻译对 + 流式 SSE 翻译处理器（沿用 sealed-failure 与有界规则）、
  错误归一化进 ai/error.rs 的固定表。
另需吸收（来自 ws4 AI-1）：failover 能力矩阵——候选带能力集，failover 跳过
不能满足请求所需能力的候选，绝不静默降级；非 chat 端点只翻译 chat + embeddings，
其余要么计量透传要么显式拒绝，不做未翻译的尽力转发。

【必读补充】
- 入口现状：edgion-gateway/src/plugins/http/ai_proxy/plugin.rs:130-146
  （端点门与 canonical 解析）
- 处理器链组装：edgion-gateway/src/ai/dispatch.rs:427-455；错误表：ai/error.rs
- 资源 schema：edgion-resources/src/resources/edgion_ai_service.rs
- 任务文档规范：skills/07-tasks/SKILL.md

【产出】tasks/ai-gateway/archive/completed-tasks/llm-ingress-breadth/ 任务包（全英文），含每个新
方言/新 provider 的 wire-contract 文档骨架，以及三个 Phase 的子任务切分。

【硬性规则】落盘内容一律英文；只写文档不改代码；不要 git commit / git push。
```

## 第 12 步：WS-02 — 实施（Phase 1 / 2 / 3 各开一个 agent）

> 用下面这块，把 `Phase N` 替换成 1 / 2 / 3。

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【任务】实施 tasks/ai-gateway/archive/completed-tasks/llm-ingress-breadth/ 的 Phase N（见其
03-subtasks.md）。设计已评审通过，不要改设计；发现问题先停下汇报。

【必读】任务包全部文档；已上线翻译器范本
tasks/ai-gateway/archive/completed-tasks/ai-gateway-anthropic-translation/04-wire-contract.md；
skills/00-design-rules.md（动代码前，提交前扫 🔴 条目）；
skills/05-testing/01-integration-testing.md。

【硬要求】
- 同方言路径（Anthropic→Anthropic、OpenAI→OpenAI）必须字节直通，不得引入任何
  翻译或重序列化——这是结构性评审标准，会被逐行检查。
- 每个翻译对交付前，先写 wire-contract 金样集成测试（先失败再实现）。
- 配额、用量、成本对新方言零感知：既有 quota / cost 场景要在新入口上重跑通过。
- Gemini 的 streamGenerateContent 流翻译处理器遵守 sealed-failure 与有界规则，
  绝不新增响应侧缓存。
- 校验：cargo fmt --all、cargo check --workspace --all-targets、clippy、
  cargo test -p edgion-gateway --lib ai::、相关集成单项。贴原始输出。

【硬性规则】落盘内容一律英文；严格 TDD；不要 git commit / git push。
```

---

## 第 13 步：WS-03 内容护栏 — 设计展开

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【前置】WS-06 的窗口原语（edgion-gateway/src/sse/window.rs）应已交付；若尚未
交付，本任务只能设计请求侧，响应侧留待窗口原语落地。

【先读】
1. tasks/ai-gateway/roadmap/03-prompt-guard-streaming.md（**范围权威**，含从 ws4
   AI-4 吸收的设计输入）
2. tasks/ai-gateway/analysis/01-data-plane-design-review.md 的 §3（请求体
   BodyStorage 与变更 API）与 §4.6（终止机制）
3. tasks/ai-gateway/archive/program-origin/04-guardrails.md（原始护栏设计，作为补充输入）

【冻结的设计决策】
- 请求侧：在已缓冲的 canonical 请求体上按角色抽取消息文本，规则求值后
  Allow / Reject（OpenAI 形状错误体）/ Mask（用 BodyStorage 既有的 replace_slice
  变更 API，它会复检物理上限）。
- 响应侧：作为 StreamingResponseProcessor 装在翻译/用量处理器之后，消费解码后的
  内容增量，维护**有界 hold-and-release 文本窗口**（约 200-256 token、约 25%
  重叠），窗口通过评估后再放行下游；命中时 Reject 走既有 terminate_ai_stream 的
  SSE 边界纪律，Mask 则改写暂扣内容后放行。
- 规则引擎：正则规则编译成 regex_automata DFA 流式匹配（复用
  plugins/http/dsl/lang/body_scanners.rs 的 StreamingRegexScanner 思路，热路径
  不用回溯引擎）；需要捕获/环视的规则只在已放行窗口内用标准 regex 求值。
- PII 规则集：内置精选正则包（覆盖面参考 agentgateway 的 PII 分类，规则自研）。
- webhook 护栏：有界外呼（build_outbound_client + tokio::time::timeout），
  失败策略是**显式 knob、默认 fail-close**（护栏不属 auth 类，按规则 10 的
  非 auth 分支）。
- 从 ws4 吸收的四条：①结构化拒绝模式（除 4xx 外支持 HTTP 200 +
  finish_reason: content_filter，流式则是 delta + stop 事件对，让 agent 链优雅
  降级）；②单一评估器契约（webhook 是唯一外部评估器接口，各家 moderation
  provider 是它背后的适配器，不做 N 个厂商插件）；③并行执行护栏 + 每护栏超时，
  链路延迟取 max 而非 sum；④PII 数据驻留路由（规则动作 block | mask | route-to，
  route-to 目标不可达时 fail-close，绝不退化成"脱敏后仍发外部"）。
- realtime WebSocket 护栏明确延后（WS 流量不走响应体管线）。

【必读补充】
- edgion-gateway/src/plugins/http/dsl/lang/body_scanners.rs（DFA 流式匹配现状）
- edgion-gateway/src/ai/usage.rs 的 terminate 机制与注入事件纪律
- edgion-gateway/src/sse/window.rs 的契约文档（WS-06 产出）
- 任务文档规范：skills/07-tasks/SKILL.md

【产出】tasks/ai-gateway/archive/completed-tasks/ai-guard/ 任务包（全英文）。01-design.md 含：
规则 schema（匹配器种类 / 作用域 / 动作 / 每规则失败策略）、内置 PII 规则集清单、
窗口参数的校验上限、三个 phase 切分（①请求侧 ②响应侧 Reject ③响应侧 Mask +
moderation 适配器）、以及延迟代价的说明（窗口会给流增加最多一个窗口的延迟，
默认关闭）。

【硬性规则】落盘内容一律英文；只写文档不改代码；不要 git commit / git push。
```

## 第 14 步：WS-03 — 实施（三个 phase 各开一个 agent）

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【任务】实施 tasks/ai-gateway/archive/completed-tasks/ai-guard/ 的 Phase N（1=请求侧、
2=响应侧 Reject、3=响应侧 Mask + moderation 适配器），按其 03-subtasks.md。
设计已评审通过，不要改设计；发现问题先停下汇报。

【必读】任务包全部文档；skills/00-design-rules.md；
skills/01-architecture/02-gateway/12-edgion-plugin-dev.md；
skills/05-testing/01-integration-testing.md。

【集成场景硬要求】请求侧 reject / mask；流中 Reject 的**干净 wire 终止**
（H1 chunked 终结符 / H2 END_STREAM，不是断连）；跨 chunk 边界的 Mask；
webhook 超时的 fail-close 与 fail-open 两态。

【硬性规则】落盘内容一律英文；严格 TDD；护栏默认关闭；窗口必须有界、eos 必冲刷；
拒绝错误体用固定文案不泄内部细节（细节进 access log）；外呼走
build_outbound_client + EgressResolver；校验命令 cargo fmt --all、
cargo check --workspace --all-targets、clippy、相关集成单项，贴原始输出；
不要 git commit / git push。
```

---

## 第 15 步：WS-05 模型路由虚拟化 — 设计与实施

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。Gateway 只构建于 Edgion 定制的 Pingora fork（Pandaala/pingora 的 edgion 分支）；
本仓库 Cargo.toml 直接依赖远端 fork。若需联调 fork 本地改动，自行加 [patch]。

【先读】
1. tasks/ai-gateway/roadmap/05-model-routing-virtualization.md（**范围权威**）
2. tasks/ai-gateway/analysis/01-data-plane-design-review.md §4.5（routing.rs 的
   5 种候选选择算法、adaptive 遥测、attempt 调度器现状）

【冻结的设计决策】
- 在 AiProxyConfig 上新增 modelRoutes：有序的模型模式规则（model 通配 + header
  匹配），动作是 rewrite（改写模型名）和/或 candidates（缩小/加权候选集）。
  failover 仍由既有调度器负责（规则里的服务集合 = 候选集，Priority 算法即
  failover 语义）。
- **不引入表达式语言**：条件路由第一阶段只做 header + model 结构化匹配
  （agentgateway 这里用 CEL，我们用结构化匹配覆盖已知用例；将来真有需求再考虑
  DSL 钩子）。
- 通配别名：per-service 别名表支持 `*` 后缀模式，按特异性排序，admission 时
  校验歧义。
- /v1/models 合成（与 WS-02 的入口端点对接）：路由候选模型的并集，按 CR 上
  per-model 的 public 标记决定可见性。
- onExceeded: Downgrade 复用 modelRoutes 的 rewrite 实现（同一套改写代码）。

【必须显式测试的三条交互】
①配额 identityModel 的桶键必须用**改写后**的模型（与现有 Downgrade 再准入行为
一致，见 plugins/http/ai_proxy/plugin.rs:362-424）；②adaptive 遥测键含模型，
必须用改写后的模型，否则 LowestLatency 数据会碎片化；③onUnknownModel:
Passthrough 只在没有任何规则命中时才生效，命中后改写的模型要正常再解析。

【任务】先在 tasks/ai-gateway/archive/completed-tasks/model-routing/ 建 feature 任务包（全英文，
含 01-design.md 与 03-subtasks.md），把设计给我过目确认后，再按 phase 实施：
①ModelRouteTable + rewrite + 通配别名（纯请求侧）②候选过滤/加权覆盖打通
routing.rs ③/v1/models 可见性集成 + 文档。

【必读补充】edgion-gateway/src/ai/routing.rs、plugins/http/ai_proxy/plugin.rs、
edgion-resources/src/resources/edgion_ai_service.rs 与
plugin_configs/ai_proxy.rs；skills/00-design-rules.md；skills/07-tasks/SKILL.md。

【硬性规则】落盘内容一律英文；严格 TDD；设计阶段只写文档；实施阶段校验命令
cargo fmt --all、cargo check --workspace --all-targets、clippy、
cargo test -p edgion-gateway --lib ai::、相关集成单项，贴原始输出；
不要 git commit / git push。
```

---

## 第 16 步：WS-07 收尾清单

```text
你在 /Volumes/ExtStore/ws4/Edgion 工作（Rust + Pingora 的 K8s 网关；分支
feature-06-24）。

【先读】tasks/ai-gateway/roadmap/07-parity-tail-and-benchmarks.md（范围权威；
注意其 7.1 已改为集成测试覆盖整合，**不做任何对照性能基准测试**——这是已关闭的
决策）。

【任务】
1. 巡检各工作流的集成场景齐全性；给 test_server 的 OpenAI mock 加一个可控 token
   速率的 SSE 流模式（服务于长流/停顿类测试，与性能测试无关）。
2. 下列每项**先问我要不要做，再动手**：7.2 prompt 富化 + 缓存标记注入 /
   7.3 tokenizer 精确预估（默认关）/ 7.4 价格目录自动刷新 / 7.5 OTel GenAI
   语义约定命名对齐 / 7.6 LiteLLM 配置导入 / 7.8 ws4 吸收的计量 backlog
   （虚拟 key + 预算层级、非 chat 端点的单位抽象计量、会话/追踪分组字段）。
3. 清理债务：tasks/ai-gateway/todo/response-body-pipeline-hardening.md 的
   D6/D8/D10 排期评估；tasks/ai-gateway/todo/open-findings/ 剩余开放 findings 的
   最终归档（已修的标 closed，并入 WS-06 的注明去向）。

【必读】上述 roadmap 文件；tasks/ai-gateway/todo/open-findings/INDEX.md；
skills/03-coding/observability/01-metrics.md（新增指标必须过
python3 cicd/checks/check_metrics_inventory.py）；skills/07-tasks/SKILL.md。

【硬性规则】落盘内容一律英文；不要 git commit / git push。
```

---

## 附：每步结束后的操作者自查清单

1. 让 agent 贴**原始校验输出**，不接受"应该没问题"这类结论。
2. 开一个新 agent 跑 `/code-review`，修完 findings 再合并。
3. 你自己确认 diff 后手动 commit / push（agent 永远不 commit）。
4. 更新本文件顶部的「当前进度」和 `tasks/ai-gateway/README.md` 的状态快照。
5. 大阶段完成后可用 `/verify` 技能起本地栈做一次端到端手工确认。
