# AI-1 - LLM Provider 抽象层

**依赖**: AI-0
**新依赖**: 纯 Rust
**预估**: 大（建议拆 1a / 1b 两阶段交付）
**对标**: Envoy `AIServiceBackend` + `BackendSecurityPolicy` / Kong `ai-proxy` /
Traefik 集中凭证管理

## 问题

这是"AI Gateway"这个词的核心含义。没有它，网关只是一个能正确代理 SSE 的普通网关。

三件事：

1. **凭证收敛** —— API key 不下发到每个调用方。客户端凭自己的身份访问网关，网关用托管
   的 provider 凭证访问上游。这是企业接入 AI Gateway 的第一动机。
2. **统一 API** —— 客户端只写一套 OpenAI 兼容请求，网关翻译成各 provider 的原生格式。
   换 provider 不改业务代码。
3. **model 虚拟化** —— 客户端请求 `model: "fast"`，网关按策略解析到具体
   provider + 具体模型；运维可以随时切换背后的实现而客户端无感。

## 竞品做法

| 网关 | 形态 |
|------|------|
| Envoy AI Gateway | `AIGatewayRoute`（按 model header 路由）+ `AIServiceBackend`（provider 定义）+ `BackendSecurityPolicy`（凭证：API key / AWS SigV4 / GCP OIDC token 交换） |
| Kong | `ai-proxy` / `ai-proxy-advanced` 插件，provider 与凭证写在插件 config |
| Traefik Hub | AI Gateway 层统一多 LLM 访问 + 集中凭证管理 |

Envoy 的三资源拆分更清晰（路由 / 后端 / 凭证各自独立、可复用），Kong 的插件形态更轻但
凭证与路由耦合。Edgion 已有 `EdgionBackendTrafficPolicy` 这类"挂在 backend 上的策略"资源，
更接近 Envoy 的形态。

## 分阶段

### AI-1a：单 provider 打通（第一阶段验收目标）

不做 schema 转换，只做直通 + 凭证 + 路由。验收：

> 客户端用标准 OpenAI SDK 把 `base_url` 指向 Edgion、本地不配任何 API key，
> 流式对话逐字吐出。

### AI-1b：多 provider schema 转换

OpenAI ↔ Anthropic ↔ Bedrock ↔ Gemini 的请求/响应双向翻译，含流式 chunk 逐块翻译。

## 需要实现

### 1. 资源模型

> **命名与形态已于 2026-08-03 收口**：资源叫 **`EdgionAIService`**（namespaced，
> `edgion.io/v1`，shortname `eais`，plugin-like archetype），路由侧用薄插件
> **`AiProxy`**（`aiServiceRef` 引用），**不**新建 `EdgionAIRoute`。
> 权威定义见 `docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md`。
> 本节以下是 AI-1b 的扩展面（AI-1a 已定的部分以 spec 为准）。

- [ ] `EdgionAIService` 在 AI-1b 需要扩展的字段：
  - `provider`: AI-1a 只有 `OpenAI` | `OpenAICompatible`；AI-1b 增加
    `Anthropic` | `AzureOpenAI` | `Bedrock` | `Vertex`
  - `endpoint`: 覆盖默认 base URL（自托管 / 代理 / Azure 部署名）—— AI-1a 已有
  - `models`: 模型列表 + 别名映射 —— AI-1a 已有
  - `secretRef`: `SecretSlot` 模式 —— AI-1a 已有；凭证池扩展见下方第 6 节
  - `defaults`: 超时、是否允许流式 —— AI-1a 已有；最大重试随 AI-3 加入
- [ ] 路由侧：model → service 的解析规则由 `AiProxy` 插件承担（AI-1a 已实现单引用形态）；
      AI-1b 需要的是**一个插件条目引用多个 service** 时的选择规则，与 AI-3 的 picker 打通
- [ ] 新资源流程参考 `skills/01-architecture/01-controller/09-add-new-resource/00-guide.md`
      的 `plugin-like` pattern
- [ ] **CRD schema 不变量**：凭证解析后的值走 conf_sync 下发时必须
      `#[schemars(skip)]` + `redact` 适配器，见 `CLAUDE.md` "CRD schema ↔ struct invariants"

### 2. model 路由

- [ ] 从请求体的 `model` 字段提取路由键（需要请求体缓存，AI-0 已提供）
- [ ] 支持别名：`model: "fast"` → `{provider: anthropic, model: claude-haiku-4-5}`
- [ ] 支持通配与默认后端
- [ ] 现成参考：`edgion-gateway/src/plugins/http/dynamic_external_upstream/plugin.rs`
      已经是"按请求内容选上游"的模式，评估能否直接扩展而不是新写
- [ ] 路由失败（未知 model）返回 OpenAI 兼容的错误体，而不是裸 404

### 3. 凭证注入

- [ ] 从 `edgion-common/src/secret/secret_store.rs` 取凭证，注入上游请求
- [ ] 三种注入方式：
  - 静态 header（`Authorization: Bearer <key>`、`x-api-key`、`api-key`）
  - **AWS SigV4 签名**（Bedrock）—— 需要对请求体做规范化签名，与 body 缓存有交互
  - **GCP / Azure OIDC token 交换**（Vertex / Azure AD）—— 需要 token 缓存与刷新，
    出站调用必须走 `build_outbound_client`（见 `CLAUDE.md` "Cross-cutting unification rules" 第 1 条）
- [ ] **必须**剥离客户端自带的 `Authorization` 头，防止客户端凭证泄漏到 provider
      或反过来绕过网关的凭证管控
- [ ] 凭证轮换：Secret 更新后热生效，不重启

### 4. schema 转换（AI-1b）

- [ ] 入口规范形态固定为 **OpenAI Chat Completions**（业界事实标准，SDK 生态最全）
- [ ] 非流式请求/响应双向转换表：messages 结构、system prompt 位置、tool/function
      calling、多模态 content block、stop reason 映射、error 结构
- [ ] **流式转换**（难点）：各家 SSE 事件模型不同
  - OpenAI: `data: {"choices":[{"delta":{...}}]}` + `data: [DONE]`
  - Anthropic: `event: content_block_delta` / `message_delta` / `message_stop` 多事件类型
  - Bedrock: `application/vnd.amazon.eventstream` 二进制帧，**不是 SSE**
  - 依赖 AI-0 的 SSE 帧解析器 + 可写管线；Bedrock 的二进制帧需要单独的解码器
- [ ] `vendor-specific` 字段透出：允许客户端通过扩展字段访问 provider 独有能力，
      不被转换层吃掉（Envoy 明确支持这点）
- [ ] 转换失败的处理：不得静默丢字段。未知字段的策略（透传 / 拒绝 / 告警）要显式配置

### 5. 配置形态草案（AI-1b：多 provider）

AI-1a 的权威形态见 spec；这里只示意 AI-1b 增加 Anthropic 这类原生 provider 后的样子。

```yaml
apiVersion: edgion.io/v1
kind: EdgionAIService
metadata:
  name: anthropic-prod
  namespace: default
spec:
  provider: Anthropic          # AI-1b 新增的 provider 之一
  secretRef:                   # SecretSlot（flatten），key 默认 api-key
    name: anthropic-key
  models:
    - name: claude-opus-5
      aliases: ["smart", "default"]
    - name: claude-haiku-4-5
      aliases: ["fast"]
  defaults:
    timeout: 300s
    streaming: true
```

## 测试

- [ ] OpenAI SDK 指向网关、本地无 key → 非流式对话成功
- [ ] 同上 → 流式对话逐字吐出，事件时序正常
- [ ] 客户端自带 `Authorization` 头 → 被剥离，不到达 provider
- [ ] 未知 model → OpenAI 兼容错误体
- [ ] 别名解析：`model: "fast"` 命中正确的 provider + 模型
- [ ] Secret 轮换后新请求用新凭证，无需重启
- [ ] （1b）同一段 OpenAI 请求分别路由到 OpenAI / Anthropic / Bedrock，客户端看到的
      响应结构一致
- [ ] （1b）流式转换：Anthropic 的多事件流被正确翻译成 OpenAI chunk 序列，
      含 tool call 分片
- [ ] （1b）SigV4 签名请求被 Bedrock 接受

## 竞品能力吸收（2026-08-03 调研）

来源与总表见 [COMPETITIVE-ABSORPTION.md](COMPETITIVE-ABSORPTION.md)。
本文件的两条落点（凭证池、非 chat 端点）主体分别写在 AI-3 与 AI-2，这里只记
**对 `EdgionAIService` 资源模型的要求**，避免实现时才发现资源结构不够用。

### 6. 凭证模型要为"凭证池"留好形状（主体在 AI-3 §6）

LiteLLM 的关键差异：一个逻辑 model 挂 N 个 deployment，**每个有自己的 API key
和独立 tpm/rpm**。对付 OpenAI 这类按 key 限流的 provider，加一把 key 就是加一份配额。

- [ ] **AI-1a 定形时就要注意**：`secretRef` 是单数形态，将来扩成"凭证集合"
      （每项带别名、独立限额、权重）时应是**新增字段**而非改语义。
      预发布无兼容负担，但资源改名/改结构会波及 CRD、文档、测试，能一次想清楚更好
- [ ] 凭证的**别名**必须是一等字段——指标与日志按别名打标，**绝不能出现 key 本身**
      （否则密钥进了可观测系统）

### 7. 跨 provider failover 的能力匹配（补充第 4 节转换层）

OpenRouter 有个务实设计：`require_parameters` —— 只路由到支持请求里全部参数的
provider。这正好回答本文件"开放问题"里那条悬着的问题（备用 provider 不支持
tool calling 某格式或多模态输入时怎么办）。

- [ ] 转换层需要一张**能力矩阵**（各 provider 支持哪些参数/特性），failover 选目标时
      先过一遍匹配；不匹配就跳过该备用，而不是发过去等它报错
- [ ] 与本文件既有结论一致：**不做静默降级**。能力矩阵是"提前跳过"，不是"砍掉参数硬发"

### 8. 非 chat 端点：翻译层的边界（主体在 AI-2 §7）

三家只翻译 chat（+embeddings），audio / images / batch / files / responses API
一律不管，用户于是绕过网关直连 provider，**治理就漏了**。

- [ ] AI-1b 的翻译范围**明确只含 chat completions**（+ embeddings，形态简单）；
      其余端点走"带计量的透传"路径（AI-2 §7），不进翻译层
- [ ] 未支持的端点必须**显式拒绝并说明**，不能静默透传成无计量的漏洞

## 开放问题

- ~~资源模型选 Envoy 三资源拆分还是 Kong 单插件形态？~~ **已收口**（2026-08-03）：
  选"一个 `EdgionAIService` 资源 + 薄 `AiProxy` 插件"，详见
  `docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md` Decision 1
  （Kong 式纯插件经认真评估后否决，理由：RBAC 归属分离、status 报错位置、
  AI-3/AI-7 挂载点；命名对齐 Traefik `AIService` / Envoy `AIServiceBackend`）
- Bedrock 的 `eventstream` 二进制帧是否值得在第一批支持？它不是 SSE，需要独立解码器，
  可以推迟到 1b 的第二轮
- 转换层放在插件里还是路由层？插件里更符合现有架构，但 model 路由发生在选上游之前，
  时序上需要确认插件阶段是否够早
- ~~备用 provider 能力不匹配时降级还是失败？~~ **已收口**：用能力矩阵提前跳过
  （上面第 7 节），不静默降级
