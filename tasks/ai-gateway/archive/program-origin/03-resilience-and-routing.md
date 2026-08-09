# AI-3 - 韧性与路由策略

**依赖**: AI-1（provider 抽象）；AI-0 提供"首字节后禁 failover"的通用边界
**新依赖**: 纯 Rust
**对标**: Kong 6 种 LLM 负载均衡算法 / Traefik 多 provider failover / Envoy provider fallback

## 问题

LLM 上游的失败模式和普通后端不同：

- **429 是常态**，不是异常。provider 的配额限制会在业务高峰稳定出现
- **失败代价高**：一次请求可能已经花了几秒和一笔钱，简单重试会翻倍成本
- **跨 provider 重试要翻译**：主 provider 是 Anthropic、备用是 OpenAI 时，
  重试不是换个地址那么简单，请求体要重新翻译一遍
- **首字节之后不能重试**：流已经开始吐字，切换上游会产生前后不一致的输出

## 需要实现

### 1. provider 级 failover

- [ ] 触发条件可配置：连接失败 / 5xx / **429** / 特定错误码 / 超时
- [ ] 备用链：`primary → secondary → tertiary`，每级可以是不同 provider
- [ ] 跨 provider failover 时请求体需重新经过 AI-1 的转换层（这是它区别于普通
      failover 的关键点，实现上要保证重试用的是**规范形态**的请求，而不是已经翻译过的）
- [ ] 依赖请求体缓存（AI-0 第 5 节）
- [ ] **硬边界**：`response_started == true` 时禁止 failover（AI-0 第 6 节已实现，
      这里只是复用并补 AI 场景的测试）
- [ ] failover 发生时的指标与日志：从哪个 provider 切到哪个、什么原因

### 2. 429 的特殊处理

- [ ] 尊重 provider 返回的 `Retry-After` / `x-ratelimit-reset-*` 头
- [ ] 主 provider 429 时优先切备用而不是等待（等待会把延迟传导给客户端）
- [ ] 可选：把 provider 的剩余配额头暴露成指标，用于容量规划

### 3. LLM 专用负载均衡

Kong 提供 6 种算法，Edgion 不必全抄。按价值排序：

| 算法 | 说明 | 优先级 |
|------|------|--------|
| `priority` | 主备优先级，主可用就不用备 | P0，failover 的自然形态 |
| `round-robin` / `weighted` | 多个等价 provider 分摊配额 | P0，现有 LB 能力直接复用 |
| `lowest-latency` | 按观测到的 TTFT 选 | P1，需要 AI-2 的指标 |
| `lowest-usage` | 按已消耗 token 选，均衡配额消耗 | P1，需要 AI-2 的计量 |
| `consistent-hash` | 按会话/用户亲和，配合 prompt caching 提高命中 | P1，现有能力可复用 |
| `semantic` | 按 prompt 语义选模型（简单问题走便宜模型） | P2，归 AI-5 |
| | | |

- [ ] 前四种在本任务实现；`semantic` 明确划归 AI-5
- [ ] 现有 LB 框架位置：`edgion-gateway/src/lb/`，评估是新增算法还是在 AI 层单独选择
      后端（倾向后者 —— provider 选择的输入是请求体内容与配额状态，和 L4/L7 端点选择
      不是一回事，塞进 `lb/` 会污染热路径）

### 4. 熔断与健康

- [ ] provider 级熔断：连续失败达阈值后暂时摘除，避免每个请求都去撞墙
- [ ] 半开探测：熔断后周期性放行少量请求探活
- [ ] 与已有的 backend 健康检查机制对齐，不要新造一套

### 5. 幂等与重试安全

- [ ] LLM 请求本身是非幂等的（有成本、可能有副作用如 tool call）
- [ ] **首版策略**：只在"未收到任何响应字节"时重试。这是安全且不需要理解语义的做法
- [ ] 重试次数硬上限，且总重试成本要计入 AI-2 的计量（一次业务调用重试 3 次 = 4 倍成本，
      不能在账上消失）

## 测试

- [ ] 主 provider 返回 429 → 自动切到备用 provider，客户端无感
- [ ] 跨 provider failover：Anthropic → OpenAI，请求体被正确重新翻译，响应结构一致
- [ ] SSE 流已推送 3 个事件后上游断开 → **不发生** failover，客户端收到中断
- [ ] 主 provider 连续失败 → 熔断生效，后续请求直接走备用而不再尝试主
- [ ] 熔断后主 provider 恢复 → 半开探测成功后回切
- [ ] `lowest-usage` 算法下，两个 provider 的 token 消耗趋于均衡
- [ ] 重试产生的额外 token 消耗被计入配额

## 竞品能力吸收（2026-08-03 调研）

来源见 `COMPETITIVE-ABSORPTION.md`。以下三项是三家（Envoy/Kong/Traefik）都没有、
但在独立 AI gateway 产品里已验证有效的能力，纳入本任务范围。

### 6. Provider 凭证池 + 感知限流的路由（吸收自 LiteLLM，权重：中）

三家路由的是**后端**，不是**凭证池**。LiteLLM 的模型是：一个逻辑 model 挂 N 个
deployment，每个 deployment 有自己的 API key 和独立 tpm/rpm；调用前先过滤掉本分钟
已超限的 deployment，失败累计到阈值后进入冷却。这对付 OpenAI 这类**按 key 限流**的
provider 极实用——加一把 key 就是加一份配额，而不必新建后端。

- [ ] `EdgionAIService` 的凭证模型从"单 secretRef"扩展为"凭证集合"，每项可带独立
      tpm/rpm 与权重
- [ ] 调用前过滤：跳过本窗口已耗尽的凭证（复用 redis 限流核心的读路径，
      不要新写计数器）
- [ ] 失败冷却：连续失败达阈值的凭证暂时摘除，与第 4 节的熔断共用状态机
- [ ] 凭证维度的用量指标（注意基数：按凭证**别名**打标，绝不能出现 key 本身）

### 7. 基于实测遥测的路由（吸收自 OpenRouter + LiteLLM，权重：中）

第 3 节的 `lowest-latency` / `lowest-usage` 只是起点。OpenRouter 的做法更完整：
先剔除近 30 秒内有故障的上游，再按价格**反平方加权**（$1/M 被选中的概率是 $3/M 的
9 倍），并允许请求级覆盖排序偏好（`sort=price|latency|throughput`）。

- [ ] 每上游维护滚动窗口统计：p50/p95/p99 TTFT、错误率、最近故障时间
- [ ] 可插拔 picker：`priority` | `weighted` | `lowest-latency` | `lowest-usage` |
      `cost-weighted`，全部读同一份统计快照
- [ ] 价格权重需要 AI-2 的价格表；在 AI-2 落地前 `cost-weighted` 不可用（配置时报错，
      不静默降级）
- [ ] 统计维护必须在热路径外（原子计数 + 周期聚合），不得在每请求做浮点排序

### 8. 预算动作不止拒绝：限速或降级模型（吸收自 TrueFoundry，权重：轻）

三家把配额当二值开关。超预算时自动降级到便宜模型（`opus → haiku`）比直接 429
对业务友好得多，尤其当调用方是 agent（收到 429 往往整条链路中断）。

- [ ] 预算状态（来自 AI-2）作为 picker 的输入之一
- [ ] 动作枚举：`reject`（默认，保持现状）| `throttle` | `downgrade`
- [ ] `downgrade` 需在配置里显式声明降级目标，不做自动猜测；降级发生时必须在
      access log 与响应头体现，不能让调用方以为拿到的是原模型的输出
- [ ] 依赖顺序：本项在 AI-2 的预算能力落地后才可实现

## 开放问题

- provider 选择逻辑放在 `lb/` 还是 AI 插件层？倾向 AI 插件层，理由见第 3 节
- 跨 provider failover 时如果备用 provider 不支持请求里用到的能力（如 tool calling
  的某种格式、多模态输入），是降级还是直接失败？倾向直接失败并明确报错，
  静默降级会产生难以排查的质量问题
- 凭证池与 provider failover 的层次关系：先在凭证池内轮换、耗尽后才跨 provider
  切换，还是两者同一层？倾向前者（同 provider 换 key 无需重新翻译请求体，代价低得多）
