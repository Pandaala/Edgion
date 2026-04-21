---
name: review
description: Review-specific knowledge for Edgion. Use when auditing code review findings, especially to check whether a reported issue is a real bug, a project-accepted tradeoff, or a known false positive. Includes observability review rules (data-plane zero tracing, metrics explosion prevention, access log quality).
---

# Review 知识沉淀

本目录存放 Edgion 项目在 review 中可复用的结论，优先记录：

- 已结合代码确认的误报或过度定级
- 需要按项目设计语义判断的问题
- 下次 review 时可直接复用的判定标准

按需读取：

- 判定内存问题时，先看 [memory-leak/not-a-memory-leak.md](memory-leak/not-a-memory-leak.md)
- 判定 CPU/Memory 性能问题时：
  - **通用方法论**（如何判断是否误报）→ [cpu-memory/performance-review-methodology.md](cpu-memory/performance-review-methodology.md)
  - **已确认的误报场景**（具体案例）→ [cpu-memory/not-a-performance-issue.md](cpu-memory/not-a-performance-issue.md)
- 审查 `RwLock`/`Mutex` `.unwrap()` 时，先看 [rwlock-unwrap/rwlock-unwrap-rule.md](rwlock-unwrap/rwlock-unwrap-rule.md)
- 审查 handler 时，先看 [handler-requeue/handler-requeue-rule.md](handler-requeue/handler-requeue-rule.md)
- 审查可观测性（日志/metrics/access log）时，先看 [observability/observability-review-rule.md](observability/observability-review-rule.md)
- 审查 TLS/SSL 客户端配置时，先看 [ssl/tonic-domain-name-false-positive.md](ssl/tonic-domain-name-false-positive.md)
- 审查 OpenSSL/rustls 后端缺口时，跳过 — 已知限制，BoringSSL 是唯一生产后端，详见 `tasks/todo/multi-tls-backend-gaps.md`
- 审查 ctx_set 模板/变量设计时，先看 [ctx-set/ctx-set-design-decisions.md](ctx-set/ctx-set-design-decisions.md)
- 审查 LB/BackendSelector 并发（ArcSwap TOCTOU、init 幂等）时，先看 [concurrency/lb-false-positives.md](concurrency/lb-false-positives.md)
<<<<<<< Updated upstream
- 审查路由架构一致性（TCP/UDP/TLS/HTTP/gRPC 跨协议对比）时，先看 [architecture/route-architecture-consistency.md](architecture/route-architecture-consistency.md)
=======
- 审查 Basic Auth"用户名枚举时序差"类 finding 时，先看 [auth-timing/basic-auth-username-enumeration.md](auth-timing/basic-auth-username-enumeration.md)——网关场景 dummy bcrypt 是反模式，已以文档方式关闭，不接受代码修复建议
- 审查 Key Auth "HashMap 查找非 CT"类 finding 时，先看 [auth-timing/key-auth-hashmap-not-ct.md](auth-timing/key-auth-hashmap-not-ct.md)——SHA-256 预处理已消除可利用性，非真实攻击面，不接受线性 CT 扫描修复建议
- 审查 ForwardAuth"默认全量转发凭证头"类 finding 时，先看 [forward-auth/default-header-forwarding.md](forward-auth/default-header-forwarding.md)——与 Traefik/Envoy 主流默认一致，URI admin 静态配置，文档已清晰说明三种模式，不接受改默认 / 强制白名单类修复建议
- 审查 OpenidConnect "`useNonce` / `usePkce` 默认关闭" 类 finding 时，先看 [oidc/nonce-pkce-defaults.md](oidc/nonce-pkce-defaults.md)——OIDC spec 定义 OPTIONAL，主流网关默认一致；服务器侧 exchange + state 已覆盖主要威胁；文档已加推荐小节。不接受改代码默认类修复建议
- 审查 Admin API "CORS 开放"/"应加来源白名单" 类 finding 时，先看 [admin-cors/admin-cors-same-origin-rule.md](admin-cors/admin-cors-same-origin-rule.md)——Edgion 采用同源部署架构（dev Vite proxy / prod dashboard 嵌入），Admin API 不挂 CORS 层是架构决策，不接受"引入 cors_allowed_origins 配置"类修复建议
- 审查 CORS 插件 "`*.domain` 简写不匹配 http"类 finding 时，先看 [cors/subdomain-wildcard-https-only.md](cors/subdomain-wildcard-https-only.md)——Edgion 自创语法糖的刻意设计，HTTPS-only 符合 operator 直觉；http 需求用 `allow_origins_by_regex` 显式声明，不接受"回退为 `https?://`"类修复建议
>>>>>>> Stashed changes

使用原则：

- 只有当结论已经结合当前代码和设计语义核实过，才写入这里
- 内容保持简短，只保留可复用判断，不复制整份 review
- 如果未来设计目标变化，要同步更新这里的结论
