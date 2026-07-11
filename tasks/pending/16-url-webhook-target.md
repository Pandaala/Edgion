# 16 - URL 目标 Webhook（外部 URL 外呼的统一基建）

> **Implemented.** See the approved design at
> `docs/superpowers/specs/2026-07-10-url-webhook-target-design.md`. Read that spec, not this file,
> for the final shape — this file is the original proposal, and the design's §2 "Findings that
> shaped the design" corrected three premises this file got wrong:
>
> 1. **§2.1** — this file's §1/§2.1 open questions assume a serde-compatibility spike is still
>    needed to find "the shared connection layer". It already existed:
>    `ExternalHttpConnection` / `OutboundRequestSpec` were already in
>    `edgion-resources/src/resources/common/`, with exactly one outbound implementation
>    (`ManagedOutboundClient` + `outbound_call`). The gap was narrower than this file assumed: only
>    `WebhookTarget` itself was pinned to a Kubernetes Service.
> 2. **§2.2** — this file's table (§1) lists LinkSys Webhook, httpdns, and wasm as the affected
>    consumers, but misses that `forward_auth` also embeds `ExternalHttpConnection` and calls
>    `validate_webhook_target`. The URL variant landed on `forward_auth` at the same time as on
>    LinkSys Webhook, with no extra work — it should have been in the original consumer list.
> 3. **§2.3** — this file's §3 "SSRF 防御" section proposes *adding* an optional gateway-local
>    policy on top of an implicit assumption that the existing per-IP guard already worked for
>    every target. It did not: `ManagedOutboundClient::dial_url`'s literal guard silently no-op'd
>    for every hostname target (pingora's `SocketAddr::from_str` fell back to Unix-socket pathname
>    parsing, so a hostname became `SocketAddr::Unix` and `classify_backend_addr`'s `as_inet()?`
>    let it through unclassified). The design's implementation repaired this (renamed to
>    `extract_ip_literal`, returns `None` instead of a bogus `Unix` address) as a prerequisite, not
>    an enhancement — shipping the URL variant on the old guard would have been an unprotected
>    egress surface.
>
> `ExternalUrlConnection` (this file's §2.2, "第二层") was explicitly **not** built by this task —
> see the design's §3 "Out of scope" and `tasks/wasm-followups/03-url-fetch-hardening.md`.

**优先级**: P1 — 横切基建，多个特性在等它
**类型**: schema 重塑 + 连接层
**依赖它的**: `tasks/wasm-followups/03-url-fetch-hardening.md`（wasm url 拉取）、
`tasks/wasm-followups/01-http-callout.md`（wasm callout 目标寻址）、
外置 DNS Part B（httpdns 的 `ExternalHttpConnection.target` 重塑遗留项）

## 0. 一句话目标

让"网关对外发 HTTP"有统一的目标寻址与连接治理：`WebhookTarget` 支持**任意外部 URL**
（不再只能指 K8s Service），并沉淀一个可内联复用的瘦连接结构，终结每个新特性各造一套
外呼配置的局面。

## 1. 背景：同一个缺口被撞了三次

| 消费者 | 现状目标寻址 | 问题 |
|--------|------------|------|
| LinkSys Webhook（正规军） | `WebhookTarget` = flatten 的 `BackendObjectReference`，**只能 K8s Service** | 外部 SaaS/制品库/第三方 API 无法作为 webhook 目标 |
| httpdns（DNS Part A） | 自带 `urlTemplate` + 尴尬挂载 `Option<ExternalHttpConnection>`（required 的 WebhookTarget 塞不进） | Part B 已记录的待重塑遗留 |
| wasm `source.url` 拉取 | 裸 URL + 裸 reqwest | 无 TLS 定制/认证/重试 |
| wasm callout（规划中） | 待定 | 需要 URL 寻址 + 允许列表 |

模式：`WebhookTarget` 当年按"外部服务一定在集群里"设计，新需求全是"任意外部 URL"。

## 2. 设计：两层互补

### 2.1 第一层 — `WebhookTarget` 支持 URL 变体（收益最大的一刀）

```yaml
# 现有形态（必须保持不破坏，存量 CR 原样反序列化）
target:
  name: auth-svc
  namespace: default
  port: 8080

# 新形态
target:
  url: https://api.example.com/v1/check
```

效果：**LinkSys Webhook 资源本身即可指外部 URL**，所有走 `webhookRef` 的消费者
（ctx_set 的 KeyGet::Webhook、DSL、access-log sink 等）零改动获得外呼外部服务能力，
`ExternalHttpConnection` 的全套治理（TLS/timeout/retry/rate-limit/health-check）自动继承。

**serde 兼容性是本任务最大的实现风险**，实现前先定方案：
- 现状是 `WebhookTarget { #[serde(flatten)] backend_ref: BackendObjectReference }`；
- 候选 a：加 `url: Option<String>` 字段 + 校验"url 与 backendRef 字段组互斥、二选一"
  （需要确认 BackendObjectReference 的 required 字段在 url-only 时能否缺省——大概率要改
  Option 化或给默认值，评估对现有反序列化路径的涟漪）；
- 候选 b：untagged enum `{ Url { url }, Service(BackendObjectReference) }`（变体顺序敏感，
  url 优先匹配；报错信息会变差，untagged 的失配报错是出了名的含糊）；
- 无论哪种：LinkSys CRD 属于 preserve-unknown-fields 的宽松簇（见 CLAUDE.md CRD 不变量），
  admission 不拦，靠 preparse 校验给 status condition —— 校验必须把"两种都填/都没填"
  说清楚。

### 2.2 第二层 — 瘦内联连接结构（给不值得建资源的一次性场景）

沉淀一个共享结构（复用**类型**不复用资源）：

```yaml
url: https://artifacts.internal/wasm/demo.wasm
tls: {...}                 # reuse TlsConfig (custom CA / mTLS)
authHeaderSecretRef: {...} # Secret-resolved auth header (redact discipline!)
timeoutMs: 10000
retry: {attempts: 3, backoffMs: 500}   # reuse WebhookRetry
```

放在 `edgion-resources/src/resources/common/`（与 external_http.rs 同级），命名建议
`ExternalUrlConnection`。消费者：wasm `source.fetch`（wasm-followups/03 从此只做接线）、
httpdns Part B 重塑、wasm callout（01）。

### 2.3 两层的关系

- 有资源级复用/共享连接配置需求 → 建 LinkSys Webhook（URL 变体）+ `webhookRef` 引用；
- 一次性/低频/嵌在自己 config 里 → 内联 `ExternalUrlConnection`；
- 两层底下共用同一个连接实现（reqwest client 构建：TLS/认证/重试），不许出现两份实现。

## 3. 横切注意事项

1. **DNS**: URL 目标意味着解析任意外部域名。第一期走系统 DNS 即可，但连接器要给
   resolver 留注入点——外置 DNS Part B/C 落地后 httpdns/自定义 resolver 能接进来，
   别到时候又对不上（同一条线的两端）。
2. **SSRF 防御**: CR 是 cluster-admin 面，基线可接受；但增加可选的 gateway 本地全局策略
   （允许域列表 / 拒绝私网段 RFC1918+link-local+metadata IP），wasm callout 必须用，
   webhook URL 变体建议默认开启拒绝 metadata 段（169.254.169.254 类）。
3. **凭据纪律**: `authHeaderSecretRef` 解析走 `EdgionPluginsHandler.parse()` / LinkSys 各自的
   Secret 解析链；resolved 值 `#[schemars(skip)]` + redact serde 适配器（`/configclient`
   输出 `[redacted]`，conf_sync 带真值）——CLAUDE.md CRD 不变量原文要求。
4. **health-check/rate-limit 对 URL 目标的语义**: 主动探测外部第三方 URL 可能不礼貌/被封，
   URL 变体下 health_check 默认关闭、显式开启才生效；rate-limit 语义不变。

## 4. 验收

- 存量 LinkSys Webhook CR 反序列化/行为零变化（回归：ctx_set webhook、DSL、access-log 现有集成测试全绿）；
- 新增：URL 变体的 LinkSys Webhook 端到端（test_server 起一个"外部"HTTP 端点）：TLS 自定义 CA、
  认证头、retry、SSRF 拒绝段命中时的行为；
- `ExternalUrlConnection` 单测 + 被至少一个消费者（建议先接 wasm source.fetch）接线验证；
- 文档：LinkSys webhook 文档加 URL 变体一节 + SSRF 策略说明；wasm-followups/03 更新为引用本任务。

## 5. 开放问题

- `WebhookTarget` 的 serde 方案（候选 a vs b）——写个 spike 分支各自跑存量 YAML 语料再定；
- SSRF 策略放 gateway 本地配置还是 EdgionGatewayConfig CRD（倾向前者起步，后者要过 CRD 覆盖守护）；
- httpdns Part B 是直接消费 `ExternalUrlConnection` 还是保留 urlTemplate 特化（模板变量替换是
  httpdns 特有的，倾向 `ExternalUrlConnection` + httpdns 自己做模板渲染后传 url）。
