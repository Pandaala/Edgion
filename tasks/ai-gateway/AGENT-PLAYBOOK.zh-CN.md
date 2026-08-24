<<<<<<< Updated upstream
• WP2 尚未进入实现阶段，当前被一个真实的 Pingora fork 前置条件阻塞。

  已完成：

  - 创建正式任务：tasks/pending/canonical-backend-runtime/canonical-backend-runtime.md
  - 完成运行时重新盘点：tasks/pending/canonical-backend-runtime/00-inventory.md
  - 完成详细设计：tasks/pending/canonical-backend-runtime/01-design.md
  - 经独立审查、两轮修改后，设计已批准：tasks/pending/canonical-backend-runtime/06-decisions.md
  - 已记录两个既有 CONNECT 测试失败，未修改 session_adapter
  - 未修改实现代码，未 commit、未 push，用户原有工作树内容均保留

  阻塞原因：当前固定及远端 edgion 分支均为 Pingora commit 480bad2a8cb85e235079968955673d63749a61a4。该版本的 HttpProxy 私有、独占持有 Connector<C>，无法让 bounded exchange 复用同一个拨号器和 H1/H2 连接池。新增 Gateway Connector 会违反“不得第二套 dialer”；Pingora
  subrequest 又会创建新 context 并重跑路由和插件。

  需要先在 Edgion Pingora fork 落地并固定以下能力：

  pub fn shared_upstream_connector(
      self,
      connector: Arc<Connector<C>>,
  ) -> Self;

  同时将 HttpProxy.client_upstream 改为 Arc<Connector<C>>。

  请提供包含该改动的新 Pingora fork commit，或明确授权我转到 Pingora fork 准备这项前置修改。固定新 commit 后，我可以从已批准设计继续实现、测试、独立代码审查及全部 quick checks。













  ### 我建议 Edgion 分成两种实现

  简单模式：Unary Callout

  SSE window
    → 一次有界 HTTP webhook/moderation 请求
    → Pass / Replace / Reject

  适合低频、窗口较大的 Guardrail。

  高性能模式：Streaming External Processor

  一个下游 SSE 请求
    → 建立一条外部处理器 H2/gRPC stream
    → window 1 → decision 1
    → window 2 → decision 2
    → ...

  类似 Envoy ext_proc，适合每个 SSE 请求需要反复检查很多窗口。
=======
# AI Gateway Agent Playbook

> The filename is retained for link stability. Repository task content is
> English-only; user-facing Chinese documentation belongs under `docs/zh-CN/`.

## Current launch order

Do not launch A2A, model virtualization, or provider-expansion code work
against the old backend identity. Use this order:

1. **Backend Phase 0/1** — create the formal Phase 1 task, freeze the nested
   `spec.ai` contract, complete independent design review, then replace the AI
   backend resource repository-wide. **Completed.**
2. **Stream Processing 01** — complete the in-progress ExtProc attached
   response stream using one request-scoped gRPC Process logical stream.
3. After ExtProc acceptance, run two ownership-safe branches:
   - **Backend Phase 2 redesign** — retain only one-time logical backend
     selection, canonical target resolution, and peer/accounting factoring with
     concrete routed-backend consumers. Do not restore the withdrawn
     selected-backend bounded exchange or shared connector.
   - **WS-06 SSE Decoder v2** — deliver the canonical bounded hold-and-release
     window without adding Guardrail behavior.
4. **Stream Processing 02** — after the SSE window is accepted, implement one
   bounded HTTP Pass/Replace/Reject decision per window for the simple
   Guardrail mode.
5. **Backend Phase 3** — generalize typed BackendTLSPolicy and
   EdgionBackendTrafficPolicy attachment with per-variant capability matrices.
6. **Backend Phase 4/5** — add `spec.a2a`, then the generic optional
   request-body observer.
7. **WS-04 A2A Phase 1** — first freeze a new Agent Card fetch transport, then
   implement card rewriting and method observation on the completed foundation.
8. Continue LLM ingress breadth, prompt guards, model routing,
   and parity-tail items according to `roadmap/00-overview.md`.

## Context to give every feature agent

- Read root `AGENTS.md`, `skills/00-design-rules.md`, and the complete
  new-feature workflow before editing.
- Treat `tasks/plan/edgion-backend-unification.md` as the authoritative
  architecture contract.
- Run `git status --short` and preserve unrelated worktree changes.
- Load architecture and external resource contracts progressively from
  `skills/01-architecture/05-resources/` and
  `skills/02-features/03-resources/`.
- Do not create compatibility aliases, duplicate resolvers/dialers, a second
  request body cache, response-side BodyStorage, or a new failure-policy shape.
- Do not commit or push without explicit user authorization.

## Stop conditions

Stop after design review and report the blocker instead of implementing when a
proposal would require any of the following:

- rerunning HTTPRoute weighting for a protocol-owned exchange;
- a plugin-local backend resolver or HTTP/TLS client;
- implicit policy inheritance from a wrapped Kubernetes Service;
- mandatory body capture for observational parsing;
- mixed old/new resource identities or binaries;
- silent acceptance of a backend variant that does not match the frontend
  protocol plugin.
>>>>>>> Stashed changes
