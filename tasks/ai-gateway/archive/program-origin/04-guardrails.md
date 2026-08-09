# AI-4 - 安全护栏 pipeline

**依赖**: AI-0（可写 + 可异步的流式管线）、AI-1（知道 provider / model）
**新依赖**: 纯 Rust + 外部 HTTP 调用（不含语义判定，语义 guard 归 AI-5）
**对标**: Traefik composable multi-vendor safety pipeline / Kong AI Prompt Guard /
AWS Bedrock Guardrails 集成

## 问题

LLM 流量的安全边界和普通 API 不同：威胁在**内容**里，不在协议里。需要在请求进入模型前
和响应返回用户前各拦一道。

Traefik 2026-03 公布的形态值得借鉴，有两个设计点很聪明：

1. **可组合、多厂商、并行执行的 pipeline** —— guard 是可插拔单元，多个 guard 并行跑，
   不是串成一条长链把延迟加起来
2. **结构化拒绝响应** —— 命中护栏时返回 HTTP 200 + 符合 schema 的拒绝消息，
   而不是 4xx。因为调用方往往是 agent，收到 HTTP 错误会中断整个 agent 流程；
   收到一条"我不能回答这个"的正常消息则可以优雅继续

第 2 点是 agent 时代的真实需求，Edgion 现有的插件拒绝机制
（`PluginRunningResult::ErrResponse`，见 `CLAUDE.md` 交叉统一规则第 8 条）默认产出的是
错误响应，需要为 AI 场景增加一种"正常响应形态的拒绝"。

## 需要实现

### 1. Pipeline 框架

- [ ] guard 单元的统一契约：输入（prompt 文本 / 响应文本 + 上下文），
      输出（`pass` | `block(reason)` | `mask(replacement)`）
- [ ] **并行执行**：多个 guard 同时跑，任一 block 即拦截。总延迟 = 最慢的 guard，
      不是所有 guard 之和
- [ ] 每个 guard 独立超时 + 独立失败策略
- [ ] **失败策略必须遵守 `CLAUDE.md` 的失败策略冻结规则**：护栏属于安全类，
      按 auth/non-auth 规则应为 **fail-close 无开关**；如果要给开关必须先对齐既有四种形状，
      不得引入第五种
- [ ] guard 执行结果进入 access log 与指标（命中了哪个 guard、什么原因）

### 2. 请求侧 guard（纯 Rust）

- [ ] **Regex guard** —— 运营方自定义正则，允许/拒绝列表（Traefik 2026-03 新增的能力）
- [ ] **关键词 guard** —— 词表匹配，比正则快，适合大列表
- [ ] **PII 检测与脱敏** —— 身份证/银行卡/邮箱/手机号正则，命中后可选 block 或 mask。
      mask 模式下发给模型的是脱敏后的文本
- [ ] **prompt 注入的结构性检测** —— 不做语义判定，只做结构性信号：
      超长重复、大段 base64、明显的角色标记伪造（`\nsystem:` 之类）
- [ ] **请求大小/轮次上限** —— messages 数组长度、单条消息长度、总字符数

### 3. 响应侧 guard（依赖 AI-0 可写流式管线）

响应侧比请求侧难一个量级，因为是流式的：内容一边生成一边吐给客户端，
等看完全文再判断就已经吐出去了。

- [ ] **增量扫描**：对已生成的文本做滑动窗口检测，命中即截断流
- [ ] **截断后的收尾**：注入一个符合 SSE / provider 格式的终止事件 +
      结构化拒绝内容，让客户端 SDK 不会因为流被硬断而报错
- [ ] **跨 chunk 边界**：敏感词可能被切在两个 chunk 之间，需要保留有界的重叠窗口
      （复用 AI-0 的跨 chunk 状态机制，同样受硬上限约束）
- [ ] 已经吐出去的内容无法收回 —— 这是流式的固有限制，必须在文档里写清楚，
      需要强保证的场景只能关闭流式

### 4. 结构化拒绝响应

- [ ] 新的拒绝形态：HTTP 200 + provider 兼容的响应体，`content` 是拒绝说明，
      `finish_reason` 标记为 `content_filter`
- [ ] 流式场景下拒绝也要用流式格式返回（一个 delta 事件 + 终止事件）
- [ ] 拒绝文案可配置（多语言、按 guard 类型不同文案）
- [ ] **必须遵守**：拒绝体不得泄漏内部细节（命中了哪条内部规则、后端地址、
      trace 内容），详情只进 access log —— `CLAUDE.md` 交叉统一规则第 9 条
- [ ] 保留传统 4xx 拒绝作为可选模式（非 agent 调用方可能更希望看到明确错误）

### 5. 外部 guard 集成

- [ ] 通用形态：把待检文本 POST 给一个 HTTP 端点，按响应决定放行/拦截
- [ ] 出站调用必须走 `build_outbound_client` + `resolve_outbound_tls` +
      `EgressResolver`（`CLAUDE.md` 交叉统一规则第 1/2/3 条）
- [ ] 响应体聚合必须有硬字节上限（第 3 条 body 不变量）
- [ ] 具体适配：AWS Bedrock Guardrails、IBM Granite Guardian（Traefik 已集成的两家）
- [ ] 现有 `ext_proc` 插件是否可以直接承担这个角色？评估后再决定是否新写

## 配置形态草案

```yaml
- type: AIGuardrails
  config:
    onBlock:
      mode: structured-refusal      # structured-refusal | http-error
      message: "This request cannot be processed."
    request:
      parallel: true
      guards:
        - type: regex
          deny: ["(?i)ignore\\s+previous\\s+instructions"]
        - type: pii
          detect: [email, phone, id-card]
          action: mask
        - type: external
          endpoint: "https://guard.internal/check"
          timeout: 500ms
    response:
      guards:
        - type: keyword
          denyList: sensitive-terms
```

## 测试

- [ ] 请求命中 regex guard → 结构化拒绝，HTTP 200，`finish_reason: content_filter`
- [ ] 同上但配置为 `http-error` 模式 → 返回 4xx
- [ ] PII mask 模式：发给上游的请求体里手机号已被替换，原文不出现在上游
- [ ] 多个 guard 并行：总延迟接近最慢的单个 guard，而非各 guard 之和
- [ ] 外部 guard 超时 → fail-close，请求被拒
- [ ] 外部 guard 返回超大响应体 → 被字节上限截断，不 OOM
- [ ] **流式响应**中途命中 guard → 流被截断，客户端收到合法的终止事件而非连接错误
- [ ] 敏感词被切在 chunk 边界中间 → 仍被检出
- [ ] 拒绝体不含任何内部规则名 / 后端地址
- [ ] 关闭 guardrails 时零额外开销

## 竞品能力吸收（2026-08-03 调研）

来源见 `COMPETITIVE-ABSORPTION.md`。

### 6. 外部 guard 做成"可插拔评估器契约"而不是 N 个专用插件（权重：中）

**这条改变第 5 节的设计方向，不是追加项。** AWS Bedrock 的 `ApplyGuardrail` 把护栏
做成了**与推理完全解耦的独立 API**：同一套策略可以评估任意模型（含自托管、第三方）的
输入与输出，调用方只需要 `bedrock:ApplyGuardrail` 权限。Azure APIM 的
`llm-content-safety` 是同一模式的另一个实例。三家（Envoy/Kong/Traefik）都是硬编码
几个固定厂商集成，而不是定义一个稳定的评估器契约——结果是每加一家就加一个插件。

- [ ] 先定义 **evaluator 契约**（请求：待检文本 + 方向(request/response) + 上下文；
      响应：`pass` | `block(reason_code)` | `mask(spans)`），再写具体适配
- [ ] Bedrock ApplyGuardrail、Azure Content Safety、自托管分类器都实现同一契约，
      配置里只是换 `type` 与端点
- [ ] 契约必须包含**流式分段评估**的语义（按语义事件/窗口批量送检，绝不逐 token 调用
      ——见 AI-0 design review 的批量规则）
- [ ] 失败策略沿用第 1 节的规则（安全类 fail-close，不引入第五种形状）

### 7. Prompt token 事前预估拦截（吸收自 Azure APIM，权重：轻，但依赖分词器）

Azure 的 `llm-token-limit` 在转发**之前**估算 prompt token 数，超配额直接拒，
不浪费一次上游调用。这是 AI-2 "事后扣减 vs 预扣结算"开放问题的**第三种答案**，
且比预扣结算简单（无悬挂预扣需要 TTL 回收）。

- [ ] 需要分词器依赖 → 归 AI-5 的依赖解锁决策；在此之前只能用字符数粗估
- [ ] 粗估模式必须明确标记，不能让粗估值进入计费口径
- [ ] 与 AI-2 的事后精确扣减配合：事前估算只用于**拦截**，事后 usage 用于**记账**

### 8. PII 感知的数据驻留路由（吸收自 Wealthsimple，权重：中）

第 2 节的 PII 检测目前只有 block / mask 两种动作。真实企业模式还有第三种:
**含敏感数据的请求路由到自托管模型（不脱敏，数据不出 VPC）；不含的才脱敏后走外部
provider**。Traefik 的 Content Guard 只做脱敏，不做路由后果。

- [ ] PII 检测结果作为路由输入之一（与 AI-3 的 picker 打通）
- [ ] 动作枚举扩展：`block` | `mask` | `route-to`（指定 EdgionAIService）
- [ ] `route-to` 的目标不可达时必须 fail-close（绝不能退回"脱敏后发外部"，
      那正是这条策略要防的事）
- [ ] 检测能力边界要在文档写清：正则/关键词只能覆盖结构化 PII（卡号、手机号、邮箱），
      自由文本里的敏感信息需要语义判定（AI-5）

## 开放问题

- 响应侧 guard 的"已吐出内容无法收回"是硬限制。要不要提供一个"缓冲 N 个 token 再放行"
  的折中模式（牺牲一点 TTFT 换取检测窗口）？倾向提供，但默认关闭
- 外部 guard 复用 `ext_proc` 还是新写插件？`ext_proc` 是 gRPC 契约，
  而 Bedrock Guardrails 是 HTTP —— 可能需要各走各的。注意第 6 节的契约化设计让这个
  问题变小：契约定好后，gRPC 与 HTTP 只是两种传输实现
