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
- 审查 DSL `log_warn(value)` / `log_error(value)` 写入 PluginLog 类 finding 时：动态值通过 `push_sensitive` 写入，行为受 `[access_log_sensitive]` 全局开关控制（默认 off → 明文，replace → `***`，wrap → `<E>...</E>`）。普通插件代码用 `push_fmt` + `sensitive(...)` 标注敏感参数。如 finding 是「应用脱敏后还能看到值」——默认 off 时正是预期行为，由部署方按合规要求开启 replace/wrap
- 审查可观测性（日志/metrics/access log）时，先看 [observability/observability-review-rule.md](observability/observability-review-rule.md)
- 审查 TLS/SSL 客户端配置时，先看 [ssl/tonic-domain-name-false-positive.md](ssl/tonic-domain-name-false-positive.md)
- 审查 OpenSSL/rustls 后端缺口时，跳过 — 已知限制，BoringSSL 是唯一生产后端，详见 `tasks/todo/multi-tls-backend-gaps.md`
- 审查 ctx_set 模板/变量设计时，先看 [ctx-set/ctx-set-design-decisions.md](ctx-set/ctx-set-design-decisions.md)
- 审查 LB/BackendSelector 并发（ArcSwap TOCTOU、init 幂等）时，先看 [concurrency/lb-false-positives.md](concurrency/lb-false-positives.md)
- 审查路由架构一致性（TCP/UDP/TLS/HTTP/gRPC 跨协议对比）时，先看 [architecture/route-architecture-consistency.md](architecture/route-architecture-consistency.md)
- 审查 `plugin_runtime.rs` "god object / 巨型文件 / 应改注册表 / 应用 enum_dispatch / 应分 stage 拆文件" 类 finding 时，先看 [architecture/plugin-runtime-god-match.md](architecture/plugin-runtime-god-match.md)——中央穷举 match 是刻意保留的设计取舍（编译期穷举检查 + 插件全景可见 + stage 约束中央化），enum_dispatch 重构对热路径性能无收益，不接受相关修复建议
- 审查 ACME `service.rs` "巨型阶段函数应拆 / 全局单例 `ACME_SERVICE` 应改 DI / handle_resource_check 太长" 类 finding 时，先看 [architecture/acme-service-shape.md](architecture/acme-service-shape.md)——ACME 协议天然线性流程 + Controller leader-elected 单活模型，拆 phase 是负价值、改 DI 收益虚；真实缺陷（吞错、UTF-8 静默降级）已修。新出现吞错 / 静默降级类才接受 finding
- 审查"文件 X 行太多应拆 / 两个模块看起来类似应抽公共 / 新增 X 自动成为对外 API"等按行数或表面相似度推动的重构 finding 时，先看 [architecture/size-and-similarity-heuristics.md](architecture/size-and-similarity-heuristics.md)——必须给出真实重复（≥2 处 30 行+ 近似块）/ 真实耦合泄漏 / 拆开后追踪成本下降 三项证据；缺一即关闭。已审计点：types/resources/common、types/mod.rs glob re-export、edgion_acme outcome 映射、rate_limit vs rate_limit_redis、runtime/store/config.rs、unified_auth/mod.rs。
- 审查 jwt_auth `load_credentials_from_secret` "4 路重复 / 应抽 helper / 函数过长" 类 finding 时，先看 [architecture/jwt-auth-credentials-source-paths.md](architecture/jwt-auth-credentials-source-paths.md)——4 路数据源类型/错误标签/asymmetric 分支行为均有结构性差异，"重复"是表象；函数被缓存只调一次，错误文案被 25 个测试逐字符校验，不接受重构类修复建议
- 审查 Basic Auth"用户名枚举时序差"类 finding 时，先看 [auth-timing/basic-auth-username-enumeration.md](auth-timing/basic-auth-username-enumeration.md)——网关场景 dummy bcrypt 是反模式，已以文档方式关闭，不接受代码修复建议
- 审查 Key Auth "HashMap 查找非 CT"类 finding 时，先看 [auth-timing/key-auth-hashmap-not-ct.md](auth-timing/key-auth-hashmap-not-ct.md)——SHA-256 预处理已消除可利用性，非真实攻击面，不接受线性 CT 扫描修复建议
- 审查 ForwardAuth"默认全量转发凭证头"类 finding 时，先看 [forward-auth/default-header-forwarding.md](forward-auth/default-header-forwarding.md)——与 Traefik/Envoy 主流默认一致，URI admin 静态配置，文档已清晰说明三种模式，不接受改默认 / 强制白名单类修复建议
- 审查 OpenidConnect "`useNonce` / `usePkce` 默认关闭" 类 finding 时，先看 [oidc/nonce-pkce-defaults.md](oidc/nonce-pkce-defaults.md)——OIDC spec 定义 OPTIONAL，主流网关默认一致；服务器侧 exchange + state 已覆盖主要威胁；文档已加推荐小节。不接受改代码默认类修复建议
- 审查 Admin API "CORS 开放"/"应加来源白名单" 类 finding 时，先看 [admin-cors/admin-cors-same-origin-rule.md](admin-cors/admin-cors-same-origin-rule.md)——Edgion 采用同源部署架构（dev Vite proxy / prod dashboard 嵌入），Admin API 不挂 CORS 层是架构决策，不接受"引入 cors_allowed_origins 配置"类修复建议
- 审查 CORS 插件 "`*.domain` 简写不匹配 http"类 finding 时，先看 [cors/subdomain-wildcard-https-only.md](cors/subdomain-wildcard-https-only.md)——Edgion 自创语法糖的刻意设计，HTTPS-only 符合 operator 直觉；http 需求用 `allow_origins_by_regex` 显式声明，不接受"回退为 `https?://`"类修复建议
- 审查"某不安全模式（如 `skip_tls`、`AllowInsecureFallback`）应在代码层面禁止/强制时限"类 finding 时，先看 [observability/system-error-state-metric.md](observability/system-error-state-metric.md)——Edgion 用 `edgion_system_error_state` gauge 登记启动期降级状态，operator 配 Alertmanager 兜底；保留紧急逃生门是产品决策
- 审查 RealIp `maxTrustedHops` "应改必填 / 改默认 / 删除" 类 finding 时，先看 [real-ip/max-trusted-hops-opt-in.md](real-ip/max-trusted-hops-opt-in.md)——opt-in 是刻意设计：默认 `None` 保持 Nginx 兼容零破坏，主防御在 `trustedIps` 精确配置，`maxTrustedHops` 是辅助防线
- 审查 ForwardAuth / Webhook "`uri` 无内网 IP 过滤 / SSRF 风险"类 finding 时，先看 [ssrf/forward-auth-webhook-ssrf.md](ssrf/forward-auth-webhook-ssrf.md)——ForwardAuth 本身就需要调用内网服务（RFC 1918 是合法目标），字面量 IP 阻断覆盖面极窄且防不了 DNS rebinding，正确防线是 K8s RBAC + NetworkPolicy + 云侧 IMDSv2，不接受代码层 IP 过滤类修复建议
- 审查 `EdgionTls` "应在 validator 拒绝 `TLS1_0` / `TLS1_1`" 类 finding 时，先看 [edgion-tls/tls-version-operator-choice.md](edgion-tls/tls-version-operator-choice.md)——为遗留客户端刻意保留的 opt-in 通道，默认不配即 TLS 1.2+，文档和 schema 多层 ⚠️ 警告；operator 知情选择，不接受代码层硬拒类修复建议
- 审查 IP 限制 / 限流 "`ClientIp` 未配置 RealIp 时可被 XFF 伪造"类 finding 时，先看 [ip-restriction/ip-source-client-ip-false-positive.md](ip-restriction/ip-source-client-ip-false-positive.md)——前提错误：`remote_addr` 未配置 RealIp 时初始化为 TCP IP，XFF 不被读取，攻击场景不成立；doc comment 已修正
- 审查 HMAC Auth "应加 nonce dedup / Redis 防重放存储" 类 finding 时，先看 [hmac-auth/replay-protection-timestamp-only.md](hmac-auth/replay-protection-timestamp-only.md)——时间戳窗口 + HTTPS + 业务层 idempotency 是 AWS SigV4 / Stripe / GitHub / Kong / APISIX 的一致分层；网关层 nonce 存储要么多实例不一致要么引入 Redis 硬依赖，不接受代码层 nonce dedup 修复建议
- 审查本地 admin JWT "logout 后仍有效 / 应加服务端黑名单 / 应改 access + refresh"类 finding 时，先看 [local-auth/jwt-revocation-stateless-tradeoff.md](local-auth/jwt-revocation-stateless-tradeoff.md)——与 Argo CD / K8s Dashboard 同模式的 stateless 取舍；需要真正撤销走 `unified_auth` 的 OIDC federation；退役运维有改密码 / 轮换 jwt_secret / 缩短 expiry 三种替代操作，不接受代码层加黑名单修复建议
- 审查 HeaderCertAuth "Header 模式单层 CA 匹配 / 未构建完整证书链" 类 finding 时，先看 [header-cert-auth/chain-verification-header-mode.md](header-cert-auth/chain-verification-header-mode.md)——Header 模式前置代理已做链验证，单层匹配是应用层二次过滤，与 Kong mtls-auth 行业一致；不接受 webpki / 链式路径构建修复建议
- 审查 HMAC Auth "未签名 header 可被篡改 / 应默认签名全部 header" 类 finding 时，先看 [hmac-auth/signed-headers-scope.md](hmac-auth/signed-headers-scope.md)——协议设计语义，与 AWS SigV4 / Stripe / Kong 一致；`enforce_headers` 配置已存在；不接受"默认全量签名"修复建议
- 审查 Redis 限流 "fail-open 应改 fail-close / 改默认为 Deny" 类 finding 时，先看 [rate-limit/redis-fail-open.md](rate-limit/redis-fail-open.md)——运维权衡，`onRedisFailure: Deny` 已可配置，Kong / APISIX 同样默认 fail-open；不接受"改默认为 Deny"修复建议
- 审查 Webhook Key Resolver "响应无签名 / 应加 HMAC 校验"类 finding 时，先看 [webhook-key/response-signature-not-needed.md](webhook-key/response-signature-not-needed.md)——Edgion 主动调外部服务属 OAuth Introspection / OPA ExtAuthz 模式（业界一致依赖 TLS 而非响应签名）；URI 由 operator 静态配置；响应签名唯一价值场景 = TLS 配错兜底，属操作失误而非代码缺口，不接受引入 `responseSignature` 字段类修复建议

使用原则：

- 只有当结论已经结合当前代码和设计语义核实过，才写入这里
- 内容保持简短，只保留可复用判断，不复制整份 review
- 如果未来设计目标变化，要同步更新这里的结论
