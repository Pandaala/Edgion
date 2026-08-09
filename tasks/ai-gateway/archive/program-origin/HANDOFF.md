# AI Gateway 第一阶段 — 执行交接书

> 给接手 agent 的唯一入口。读完本文件即可开工,按「执行顺序」推进,不要跳步。
> 创建:2026-08-03(设计会话)。若本文件与 spec 冲突,以 spec 为准。

## 一、目标与验收

让 Edgion 网关具备 AI Gateway 第一阶段能力。**唯一验收标准**:

> 客户端用标准 OpenAI SDK 把 `base_url` 指向 Edgion、本地不配任何 API key,
> 流式对话逐字吐出,网关侧出 token 用量指标(metric + access log 字段)。

第一阶段 = **AI-0 响应侧流式地基** + **AI-1a 单 provider 直通**(无 schema 转换)。

## 二、必读文件(按优先级)

| 文件 | 作用 |
|---|---|
| `docs/superpowers/plans/2026-08-03-ai0-response-streaming-foundation.md` | **AI-0 实施计划,当前执行入口**。5 个任务,每个带完整代码/测试/验证命令/checkbox |
| `docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md` | 第一阶段设计 spec(已评审)。AI-1a 的唯一权威;所有"为什么"在这里 |
| `tasks/todo/ai-gateway/README.md` | 程序级路线图(AI-0~AI-7 依赖图、cross-cutting 约束) |
| `tasks/todo/ai-gateway/00-streaming-foundation.md` | AI-0 全景 + 底部 "Status 2026-08-03" 小节(哪些已随 fork 落地) |
| `CLAUDE.md`(仓库根) | 项目硬约束(🔴 规则、body 不变量、统一点、git 规则) |

## 三、现状(2026-08-03 快照,开工前用 git log 复核)

**已完成,不要重做:**

- AI-0 **请求侧**已随 pingora fork 落地:fork 锁定 `b1d520e`(canonical request-body
  transport interface:`request_body_filter_action`/`Terminate`/disposition/retry 判定)
  + `b829a34`(request body cache)。Edgion 侧接线在
  `pg_request_body_filter_action.rs`、`BodyStorage`(实现 fork 的 `RequestBodyBuffer`)、
  body gate(R1–R23 admission model)。AI-0a 调研门禁已关闭。
- 设计与计划文档(上表)已定稿;路线图已从旧的 `llm-ai-gateway/` 等目录收口到本目录。

**待做(= 你的工作):**

- AI-0 响应侧 5 项(见执行顺序第 1 步)。
- AI-1a(见执行顺序第 2、3 步)。

**关键代码事实**(计划里有细节,这里只列避免误判的):

- 响应体插件契约目前只读:`&mut Option<Bytes>` 在
  `pg_upstream_response_body_filter.rs` 传入插件运行时时被降级为 `&`。恢复可写只改
  Edgion,不动 fork。
- 全仓库目前零 SSE 处理、零流式响应识别。
- `enable_compression: true` 存量 bug:true 分支根本不安装压缩模块(AI-0 计划 Task 1 修)。
- fork 的 `Terminate` 只覆盖请求体;**响应流截断 fork 还没有**,属 AI-4,本阶段不做。
- 响应体缓冲(`read_response_body`)仍 `Unimplemented`,本阶段不需要;遗留事项在
  `tasks/todo/response-body-buffer-runtime.md`,别顺手实现。

## 四、执行顺序

### 第 1 步:执行 AI-0 计划(现在就能开工)

打开 `docs/superpowers/plans/2026-08-03-ai0-response-streaming-foundation.md`,
从**第一个未勾选的任务**开始,严格按任务内步骤(TDD:先写失败测试)执行:

1. Task 1 压缩模块安装修复
2. Task 2 流式识别(`ResponseStreamKind`)+ 逐响应压缩豁免
3. Task 3 `StreamingResponseProcessor` trait + ctx 管线
4. Task 4 有界 SSE 解码器(`edgion-gateway/src/sse/`)
5. Task 5 集成覆盖(test_server `/sse` 端点 + `SseStreaming` 场景)

协议:每完成一步就把计划里对应 `- [ ]` 勾成 `- [x]`(这是跨会话进度状态);
每个任务验证 = `cargo fmt --all` + `cargo check`(Task 5 用 `--workspace --all-targets`)
+ 该任务的定向测试。

### 第 2 步:写 AI-1a 实施计划(AI-0 全绿之后)

以 spec 为唯一依据,写 `docs/superpowers/plans/` 下的 AI-1a 计划(格式对齐 AI-0 计划)。
spec 中已定死、不得重开的决策:

- 资源:**`EdgionAIService`**(namespaced,`edgion.io/v1`,shortname `eais`,
  plugin-like archetype,凭证用 `SecretSlot` 模式)。命名对齐 Traefik `AIService` /
  Envoy `AIServiceBackend`;"要不要建资源"已经过两轮讨论定案,理由在 spec Decision 1,
  不要再改成纯插件形态。
- 插件:**`AiProxy`**(request 阶段,`aiServiceRef` 引用资源,intrinsic fail-close
  `body_requirement`,剥离客户端 `Authorization`,注入 provider 凭证,
  `set_upstream_jump(External)` 复用 `dynamic_external_upstream` 模式)。
- provider 范围:`OpenAI` + `OpenAICompatible`,零 schema 转换。
- usage 提取:流式(SSE 最后事件,默认注入 `stream_options.include_usage`)+
  非流式(≤1 MiB 有界累积);缺失则 chars/4 估算并打 `estimated` 标。
- 指标:`edgion_gateway_genai_tokens_total{direction,provider,model}`(model 标签只认
  EdgionAIService 声明过的名字,其余归 `other`;必须同步更新
  `docs/en/ops-guide/observability/metrics-inventory.md`,CI 强制)。TTFT 只进 access log,
  不加 histogram。
- 错误一律 OpenAI 兼容错误体,不泄内部细节(映射表在 spec 的 Error handling 一节)。

新资源走 `skills/01-architecture/01-controller/09-add-new-resource/00-guide.md` 全清单,
CRD 加入 `crd_coverage.rs` 守卫(第四个结构化 CRD)。

### 第 3 步:执行 AI-1a 计划 → 跑验收

验收即第一节的场景;集成场景复用 AI-0 Task 5 的 mock 基建扩展成 OpenAI mock。
全部通过后更新 `tasks/todo/ai-gateway/README.md` 状态,并把两份计划的完成情况回写。

## 五、硬性约束(违反即返工)

1. **不要自行 `git commit` / `git push`**——每个任务完成后向用户汇报,由用户提交。
2. 落盘内容一律英文(代码/注释/测试/文档;`docs/zh-CN/` 除外)。
3. 数据面禁 `tracing::warn!/error!/info!`;响应体热路径禁逐 chunk 日志/PluginLog,
   聚合后在请求结束时一次性输出。
4. 有界状态:SSE 半帧上限 64 KiB,超限永久降级透传,绝不无界累积;不做
   Envoy 式 pause/resume;pump 独占流的所有权。
5. 流式识别只认显式媒体类型(`text/event-stream`);无 Content-Length / chunked
   不算流式。
6. 子任务验证只跑 fmt + check + 定向测试;**不跑** `cargo clippy` / `cargo build`
   (clippy 在全部任务完成后由主会话统一跑一次)。
7. 预发布无兼容负担:可自由改名/改签名,不留 shim;但不做未要求的重构。
8. 提交前对照 `skills/00-design-rules.md` 的 🔴 行扫一遍 diff。

## 六、命名消歧(历史坑)

- "方案 A/B/C" = 资源形态讨论(A=建 EdgionAIService ✅ 已选;B=Kong 纯插件 ❌;
  C=Envoy 三资源 ❌)。
- "AI-0 计划 / AI-1a 计划" = 两份实施计划(曾叫 Plan A/Plan B,已弃用该叫法)。
- 二者无关,不要把"AI-1a 计划"误读成"方案 B"。

## 七、第一阶段之后(不要提前做)

依赖顺序:AI-1b(schema 转换)→ AI-2(计价/token 限流)→ AI-3(多 provider 韧性)
→ AI-4(护栏,需先给 fork 加响应流 Terminate)→ AI-5(语义层,需解锁新依赖)。
AI-6(MCP)独立支线;AI-7(推理路由)依赖 AI-1。各自任务文件在本目录。
