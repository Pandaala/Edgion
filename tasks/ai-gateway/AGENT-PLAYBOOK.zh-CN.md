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
