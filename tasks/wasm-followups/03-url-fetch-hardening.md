# 03 - url 来源拉取强化（对齐 webhook 级连接治理）

**优先级**: P2（02 决策后启动；若 02 确认 url 为中期主推形态则升 P1）
**类型**: 强化
**前置**: `tasks/pending/16-url-webhook-target.md` (design: `docs/superpowers/specs/2026-07-10-url-webhook-target-design.md`).
Task 16 shipped: the `WebhookTarget` URL variant (origin-only external URL addressing on the
LinkSys Webhook resource and, for free, on `forward_auth`) and the outbound egress guard
(`EgressResolver` + `blockPrivate`, gateway-local `outbound_egress_block_private` default). Task 16
explicitly did **not** ship `ExternalUrlConnection` — that thin inline connection struct remains
this task's responsibility to define. Derive its shape from the two real consumers that need it:
wasm `source.fetch` (this task) and wasm callout (`tasks/wasm-followups/01-http-callout.md`), not
by guessing ahead of a consumer.
**核心问题**: 裸 reqwest 升级为带 TLS/认证/重试的可靠拉取——桥接 LinkSys，还是精简连接结构？

## 现状

`vm_manager.rs::fetch_bytes` url 分支：`reqwest::Client` + 10s 超时 + 仅 2xx + 强制 sha256 校验，
加载失败缓存 30s 后可重试。缺：自定义 CA / mTLS、认证头（私有制品库必需）、重试/退避、
代理支持。

## 两个候选方向

### 方向 A：桥接 LinkSys（`linkSysRef`）

WasmConfig 加 `source.linkSysRef: {namespace, name}`（复用现成 `LinkSysRef` 结构，DSL/access-log
已有同款引用模式），由 LinkSys 资源承载连接治理（`ExternalHttpConnection`：TLS、timeout、retry、
rate-limit、health-check 全套现成）。

- 优点：零新 schema，连接治理能力最全，凭据解析链（Secret）现成。
- 缺点：**已知结构不匹配**——`ExternalHttpConnection.target` 是 `WebhookTarget`（指 K8s Service
  backend），而 wasm url 是任意外部 URL。这正是外置 DNS Part B 记录过的同一个遗留问题
  （httpdns 也是 urlTemplate 对不上 WebhookTarget，见 `.superpowers` DNS ledger 的
  DEFERRED 项）。桥接前必须先做 target 重塑，否则是硬塞。
- 还引入：LinkSys 资源生命周期依赖（ref 追踪、requeue）、一次模块拉取用不上
  rate-limit/health-check 这类长连接治理（过重）。

### 方向 B：精简连接结构（推荐）

`WasmConfig` 加一个瘦的 `source.fetch` 子结构（仅 url 形态生效）：

```yaml
source:
  url: https://artifacts.internal/wasm/header-demo-1.2.0.wasm
  fetch:                       # optional; defaults keep current behavior
    tls:                       # reuse the existing TlsConfig type (custom CA / mTLS)
      caCertSecretRef: {...}
    authHeaderSecretRef:       # Secret carrying e.g. "Authorization: Bearer <token>"
      name: artifact-repo-token
      key: authorization
    timeoutMs: 10000
    retry: {attempts: 3, backoffMs: 500}   # reuse WebhookRetry shape
sha256: "..."
```

- 复用**类型**（`TlsConfig`、`WebhookRetry`）而不是复用**资源**——拿到治理能力，不背 LinkSys
  的资源耦合。
- `authHeaderSecretRef` 走 controller 的 Secret 解析链（`EdgionPluginsHandler.parse()` +
  `secret_ref_manager` 级联注册，插件开发指南 §6 有完整先例）——注意 resolved 凭据字段要
  `#[schemars(skip)]` + redact 纪律（CLAUDE.md 的 CRD 不变量）。
- 与 01（HTTP callout）共享同一套外呼结构：callout 的目标连接治理直接复用 `fetch` 的
  tls/timeout/retry 形状，避免两套。

### Recommendation (updated: task 16 did NOT define `ExternalUrlConnection`)

Direction B still stands, but the ownership note above is stale as of the task-16 design
(`docs/superpowers/specs/2026-07-10-url-webhook-target-design.md`, §3 "Out of scope"): task 16
deliberately did **not** define `ExternalUrlConnection`. Reusing `TlsConfig` / `WebhookRetry` is
still the right call, but this task derives the struct's own shape — informed by its two real
consumers, wasm `source.fetch` (this task) and wasm callout (task 01) — rather than inheriting a
shape task 16 was asked to guess ahead of either consumer existing. The remaining scope is
unchanged: wasm `source.fetch` wired to the new `ExternalUrlConnection`, plus the wasm-side
implementation notes and acceptance criteria below.

Direction A (`linkSysRef`) is now a genuinely usable alternative, not merely "unblocked in theory":
task 16 shipped the `WebhookTarget` URL variant, so a LinkSys Webhook can already point at an
arbitrary external URL (origin-only, with its own connection pool and egress guard) and be
referenced by `linkSysRef` today. It is still not pursued in this task — a one-shot wasm module
fetch does not need the LinkSys resource's rate-limit/health-check governance — but it is no longer
blocked on a target reshape.

## 实现要点

- 重试只对网络错误/5xx，4xx 不重试（认证错误重试无意义且刷爆制品库日志）；
- 重试预算记入现有的 30s 负缓存语义（重试全失败才进负缓存）；
- sha256 仍强制（不因为有了 TLS 就放松完整性校验）；
- 拉取并发限制（多个插件同 url 已有 OnceCell 去重；不同 url 并发加个小上限防启动风暴）。

## 验收

- 单测：认证头注入、自定义 CA 握手、重试次数/退避、4xx 不重试、负缓存交互；
- 集成：test_server 加一个带 Bearer 校验的制品端点，url 源全链路（含 401→fail、带 token→成功）；
- 文档 wasm.md url 一节更新 + redact 检查（`/configclient` 输出不得出现凭据明文）。
