# AI-2 - 计量与成本治理

**依赖**: AI-0（流式读取管线）、AI-1（知道请求打到哪个 provider / model）
**新依赖**: 纯 Rust（`usage` 字段由 provider 返回；事前预估拦截需要分词器，归 AI-5）
**对标**: Kong `ai-rate-limiting-advanced` / Traefik token 级成本控制 / Envoy token-aware rate limiting

## 问题

LLM 流量的成本模型和普通 HTTP 完全不同：一次请求的代价差异可达三四个数量级，按"请求数"
限流毫无意义。企业接入 AI Gateway 的第二动机就是**看得见、管得住花了多少钱**。

三层能力，从易到难：

1. **计量** —— 每次调用消耗了多少 token，属于谁
2. **观测** —— 指标 + 审计日志
3. **管控** —— 按 token 的限流与预算配额

## 现状

- 限流核心已有：`edgion-gateway/src/plugins/common/redis_rate_limiter`（Lua 实现）
  + `edgion-gateway/src/plugins/http/rate_limit_redis/`
- Redis 访问必须走 LinkSys `RedisLinkClient`（`CLAUDE.md` 交叉统一规则第 5 条）
- 缺的是"成本"这一维：现有限流每次请求扣 1，token 限流需要扣一个**运行时才知道的动态值**，
  而且这个值**要等响应结束才知道**

## 需要实现

### 1. usage 提取

- [ ] 非流式：从响应体 JSON 抽 `usage.prompt_tokens` / `completion_tokens` / `total_tokens`
- [ ] **流式**：`usage` 通常只出现在流的最后一个事件（OpenAI 需要请求里带
      `stream_options: {include_usage: true}` 才会返回；Anthropic 在 `message_delta`
      事件里给 `output_tokens`）
  - 依赖 AI-0 的 SSE 帧解析器逐帧扫描
  - **必须处理 provider 不返回 usage 的情况**：降级到粗估（字符数 / 4）并打标，
    指标上区分"精确"与"估算"，不能让估算值混进计费口径
- [ ] 各 provider 的 usage 字段名与位置差异表（OpenAI / Anthropic / Bedrock / Gemini）
- [ ] 缓存命中、工具调用轮次等场景的 token 归属规则要写清楚（prompt caching 会让
      `cache_read_input_tokens` 单独计费）

### 2. 指标

采用 OpenTelemetry GenAI 语义约定，避免自造一套：

- [ ] `gen_ai.client.token.usage{gen_ai.operation.name, gen_ai.system, gen_ai.request.model, token.type}`
      —— `token.type` 区分 `input` / `output`
- [ ] `gen_ai.server.time_to_first_token`（TTFT）—— 流式体验的核心指标
- [ ] `gen_ai.server.time_per_output_token`
- [ ] `gen_ai.client.operation.duration`
- [ ] **基数控制**：`model` 由客户端指定、可能无界，必须有 allowlist 或 top-N 之外归
      `other`。见 `cicd/checks/check_metrics_inventory.py` 的高基数标签守卫 —— 新增指标
      必须同步更新指标清单文档，否则 CI 直接失败

### 3. 成本审计日志

- [ ] 每次调用一条结构化记录：调用方身份、model、provider、input/output token、
      估算金额、是否流式、是否命中缓存、耗时、TTFT
- [ ] 价格表配置化（`per 1M input tokens` / `per 1M output tokens`，按 model 粒度），
      价格变动不改代码
- [ ] 金额只做**估算**，日志字段名要体现这点（`estimated_cost_usd`），
      不能被当作账单依据
- [ ] 落在 access log 还是独立 audit sink？倾向复用 access log 的结构化字段，
      避免第二套日志管道（`CLAUDE.md` observability restraint）

### 4. token 限流与预算

- [ ] 扩展 `redis_rate_limiter` 核心：允许扣减一个运行时决定的**动态成本**，
      而不是固定 1
- [ ] **时序问题**：token 数原本要等响应结束才知道。**但流式响应可以边流边算**
      （2026-08-03 决策，见下方"流式增量扣减"），这改变了原来的策略选择
#### 流式增量扣减（首版策略，2026-08-03 定）

原方案在"事后扣减"与"预扣结算"之间二选一，两者都忽略了一个事实：**AI-0 的 SSE 解码器
已经在逐帧解流，输出 token 完全可以边流边算**，不必等 `usage` 事件。这一条同时取代
了原来的两个选项：

- 相比事后扣减：超支上限从**一次完整调用**（一个 100k token 的响应可以一次击穿配额）
  降到**一个批次窗口**
- 相比预扣结算：达到同等的超支控制，却不需要处理连接中断造成的悬挂预扣与 TTL 回收
- 额外好处：**客户端中途断流天然正确**——已生成部分已经扣掉，无需补追机制

- [ ] **本地累加 + 批量刷新**（硬约束）：绝**不得**逐 token / 逐 chunk 写 Redis
      —— AI-0 design review 的批量规则。形态是：每帧解出后在进程内累加（纯内存零成本），
      每累计 N 个 token 或每 T 毫秒刷一次 Redis；超支上限 = N，可配置（如 N=100）
- [ ] **信号分层**：流中用可得的最好信号，各 provider 不同——
      Anthropic 的 `message_delta` 带**累计** `output_tokens`（provider 权威值，直接用）；
      OpenAI 流中途不给数，只能用 "chunk 数 ≈ token 数" 近似（通常一 chunk 一 token，
      但不保证）
- [ ] **流末对账**：拿到权威 `usage` 后与增量累计值比对，向 Redis 补写 delta；
      **日志与指标一律记权威值**，增量值只用于执行。这样执行实时、账目准确
- [ ] **能力边界（必须写进文档）**：增量**计量**现在就能做；但配额耗尽时**掐断正在吐的流**
      需要响应流 terminate 能力，fork 目前只有请求侧 `Terminate`，响应侧随 AI-4 一起上。
      在那之前流中途超限只能"扣成负数、下一次请求拒绝"，不能中断当前这条——
      不要让运维以为配了就能硬拦
- [ ] 非流式响应仍走响应结束后一次性扣减（无中间信号可用，也无必要）

#### 存储分层（2026-08-03 定）

Redis 与日志的职责必须分清，否则会把缓存当账本：

- **Redis = 执行层**：多实例共享的窗口计数器，只为"这次请求放不放行"服务。
  它是**派生值/缓存**，丢了可以从日志流重建
- **结构化 access log → ES/ClickHouse = 权威账本**：耐久、可审计、可重放、可对账。
  计费口径只认它
- 两者对不上时**以日志为准**；不要为了让 Redis "准确"而给它加持久化职责
- Redis 访问走 LinkSys `RedisLinkClient`，限流 Lua 扩展共享的 `redis_rate_limiter` 核心
  （交叉统一规则第 5 条）——不新写第二套
- Redis 不可用时的失败策略：限流属非 auth 类 → 显式开关，**默认 close**
  （失败策略冻结规则）
- 延迟不是问题：LLM 请求以秒计，Redis 往返亚毫秒级，占比可忽略；且增量刷新发生在
  响应流处理中、最终对账发生在 logging 阶段，都不加进客户端可感知延迟

- [ ] 限流维度：调用方身份（来自认证）/ model / provider / 自定义 header
- [ ] 支持组合规则：如"某团队每天 10M token，其中 opus 模型每天 1M"
- [ ] 超限响应用 **OpenAI 兼容的错误体** + `429` + `Retry-After`，让 SDK 能正确重试
- [ ] 预算配额（长窗口，如按月）与限流（短窗口）共用核心但语义不同：配额耗尽通常要
      **持续拒绝**而不是滑动恢复

## 配置形态草案

```yaml
- type: AITokenRateLimit
  config:
    redisLink: ai-quota
    pricing:
      - model: claude-opus-5
        inputPerMillion: 15.0
        outputPerMillion: 75.0
    rules:
      - dimension: consumer          # 来自认证主体
        limit: 10000000              # token
        window: 24h
        strategy: post-deduct        # post-deduct | pre-reserve
      - dimension: consumer
        model: claude-opus-5
        limit: 1000000
        window: 24h
```

## 测试

- [ ] 非流式响应的 usage 被正确提取，指标 label 正确
- [ ] 流式响应的 usage 从最后一个事件提取成功
- [ ] provider 未返回 usage → 降级估算，且指标上标记为估算
- [ ] 客户端中途断开的流 → 已产生的 token 仍被计量（不能漏计）
- [ ] token 限流：连续调用直到超限，第 N+1 次被 429 且错误体是 OpenAI 兼容格式
- [ ] **流式增量扣减**：一次长响应过程中 Redis 计数**分多次**上升（而非结束时一次跳变），
      且刷新次数 ≈ 总 token 数 / N，不是每 token 一次（防止退化成逐 token 写）
- [ ] **流末对账**：增量近似值与权威 `usage` 不一致时，Redis 被补写 delta，
      日志记的是权威值
- [ ] 流式响应中途客户端断开 → 已生成部分已计入配额，无需补追
- [ ] 高基数保护：1000 个不同 model 名不会让指标基数爆炸
- [ ] 关闭计量时对普通 HTTP 流量零额外开销（对比压测）

## 竞品能力吸收（2026-08-03 调研）

来源见 `COMPETITIVE-ABSORPTION.md`。**本任务是三家（Envoy/Kong/Traefik）共同最弱的
一轴**——它们都是无状态 CRD 代理，没有租户库，Envoy 到 v1.0 还把"美元级成本管控"
列在 roadmap。而这恰是企业采购 AI gateway 的第二动机（第一是凭证收敛）。

### 5. 虚拟 key + 组织/团队/用户三级预算与消费追踪（吸收自 LiteLLM，权重：重）

LiteLLM 的核心差异化：DB 支撑的**虚拟 key** —— 调用方拿到的不是 provider key，而是
网关签发的虚拟 key，其上挂载预算、限流、可用模型范围，并按 key/user/team/org 四级
聚合消费。key 可轮换（带宽限期）。

- [ ] **先做架构决策再动手**：这要求网关侧有持久化状态（消费累计、key 元数据），
      与 Edgion 现有"无状态数据面 + 控制面下发配置"的模型冲突。三条路径：
      (a) 用既有 LinkSys Redis 做消费累计 + 用 CRD 定义 key（配置态），
      (b) 引入独立的 admin store（重，需要评估是否值得），
      (c) 只做消费**观测**（access log 聚合到 ES/ClickHouse），预算靠外部系统回写限流规则。
      **倾向 (a)**：与既有 redis 限流核心同源，CRD 定义 key 符合现有资源模型；
      (c) 是最轻的过渡形态
- [ ] 虚拟 key 与既有 `KeyAuth` / `JwtAuth` 插件的关系必须先理清——**不要新造第二套
      认证机制**，虚拟 key 应该是"认证主体 + 预算元数据"的组合，认证仍走既有插件
- [ ] 四级聚合（key/user/team/org）在首版可以只做两级（key + 归属组），
      层级深度不是价值所在，消费可见性才是
- [ ] 价格表（见第 3 节）是本项的前置

### 6. 预算动作不止拒绝（吸收自 TrueFoundry，权重：轻）

超预算时可选 `throttle` 或**降级到便宜模型**，而不只是 429。实现落在 AI-3 的 picker
（见 `03-resilience-and-routing.md` 第 8 节），本任务只需把预算状态暴露成 picker 的
可读输入。

### 7. 非 chat 端点的统一治理（吸收自 LiteLLM，权重：中偏重）

三家只翻译 chat（+embeddings），其余端点（audio / images / batch / files /
responses API）用户会直接绕过网关调 provider，**治理就漏了**。

- [ ] 首版目标不是翻译，而是**带计量的透传**：识别端点类型，用同一套凭证与限流，
      按端点各自的计费单位记账（audio 按秒、images 按张，不是 token）
- [ ] 计量单位抽象要在 AI-2 一开始就留好，不要把 `token` 硬编进数据模型
      （否则加 audio 时要改所有存储与指标）
- [ ] 不支持的端点必须**显式拒绝并说明**，不能静默透传导致无计量的漏洞

### 8. 会话/追踪分组 + 载荷日志（吸收自 Helicone / Cloudflare，权重：中）

agent 场景的调试单元是**工作流**而不是单个请求。Helicone 的 session 树把一条 agent 链
的多次 LLM 调用 + 工具调用聚合成一棵树，带整链成本与延迟。

- [ ] 从请求头取会话/追踪 ID（沿用 W3C traceparent 或可配置头），
      写进 access log 的结构化字段，聚合留给 ES/ClickHouse
      （符合"优先 access log 后聚合"的既有偏好，不在进程内维护会话树）
- [ ] 全量载荷（prompt/completion）日志是**默认关闭的可选能力**，
      开启时必须有独立开关 + 明确的合规提示；绝不能与常规 access log 混在一起
      （见 cross-cutting constraints：敏感内容不进普通日志）
- [ ] 行业信号：Helicone 已于 2026-03 被 Mintlify 收购转维护模式——独立观测代理正在
      被网关吸收，这类能力本就该长在网关里

## 开放问题

- ~~事后扣减 vs 预扣结算~~ **已收口**（2026-08-03）：首版 = **流式增量扣减 + 流末对账**，
  见第 4 节。预扣结算被否决（复杂度换来的超支控制，增量扣减用更简单的方式已达到）。
  仍开放的是**事前预估拦截**（Azure APIM 模式，见 `04-guardrails.md` 第 7 节）要不要叠加：
  它拦的是**输入侧**（超大 prompt 在转发前就拒），与增量扣减拦输出侧互补；
  但精确预估需要分词器（AI-5 依赖决策），在此之前只能字符数粗估
- 成本审计日志要不要单独的 sink（便于对接计费系统）？还是坚持只走 access log？
- 多轮工具调用（agent 场景）的一次"会话"跨多个 HTTP 请求，配额是否需要按会话聚合？
  这会把 AI-6 的 session 概念引进来，需要评估是否值得
- 虚拟 key 的状态存储路径（上面第 5 节的 a/b/c）是本任务最大的架构决策，
  应在动手前单独设计评审
