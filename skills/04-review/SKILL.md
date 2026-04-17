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
- 判定 CPU/Memory 性能问题时，先看 [cpu-memory/not-a-performance-issue.md](cpu-memory/not-a-performance-issue.md)
- 审查 `RwLock`/`Mutex` `.unwrap()` 时，先看 [rwlock-unwrap/rwlock-unwrap-rule.md](rwlock-unwrap/rwlock-unwrap-rule.md)
- 审查 handler 时，先看 [handler-requeue/handler-requeue-rule.md](handler-requeue/handler-requeue-rule.md)
- 审查可观测性（日志/metrics/access log）时，先看 [observability/observability-review-rule.md](observability/observability-review-rule.md)
- 审查 TLS/SSL 客户端配置时，先看 [ssl/tonic-domain-name-false-positive.md](ssl/tonic-domain-name-false-positive.md)
- 审查 OpenSSL/rustls 后端缺口时，跳过 — 已知限制，BoringSSL 是唯一生产后端，详见 `tasks/todo/multi-tls-backend-gaps.md`
- 审查 ctx_set 模板/变量设计时，先看 [ctx-set/ctx-set-design-decisions.md](ctx-set/ctx-set-design-decisions.md)
- 审查 LB/BackendSelector 并发（ArcSwap TOCTOU、init 幂等）时，先看 [concurrency/lb-false-positives.md](concurrency/lb-false-positives.md)

使用原则：

- 只有当结论已经结合当前代码和设计语义核实过，才写入这里
- 内容保持简短，只保留可复用判断，不复制整份 review
- 如果未来设计目标变化，要同步更新这里的结论
