# AI-0 - 流式与请求体地基

**依赖**: 无
**新依赖**: 纯 Rust
**收益**: 一次性解决 MCP、OpenAI/Anthropic 流式 API、A2A 等全部 AI 流量的基础可用性；
并为 AI-1/AI-2/AI-4 提供它们共用的流式处理管线
**来源**: 吸收原 `mcp/01-sse-streaming-passthrough.md` 全文 + `mcp/03` 的通用部分

> **Investigation gate:** Do not implement the writable streaming pipeline in
> this task until
> [AI-0a](00-body-stream-pingora-investigation.md) traces the pinned Pingora
> fork's H1/H2 pumps and recommends the minimum transport-boundary change.
> Earlier statements in this document that the work requires no Pingora change,
> or that it should reuse request `BodyStorage`, are not design decisions.
> `BodyStream` is an independent online transformation mechanism; `BodyStorage`
> remains the complete-request materialization and replay mechanism.

## 问题

本任务有两个面，必须都做。只做第一面就是原 `mcp/01` 的范围，不足以支撑后续任何 AI 能力。

### 消极面：别把流式弄坏

SSE 响应（`Content-Type: text/event-stream`）的语义是**服务端逐个事件推送、客户端立即
消费**。链路上任何一处缓冲都会让客户端在事件到达前一直挂起，表现为"AI 不回复"或"回复
一次性全部吐出"。普通 HTTP 代理的常见优化在流式场景全是反模式：响应压缩要攒缓冲区、
缓存要读完整个 body、body 插件要累积后再改写。

### 积极面：要能在流式上做事

后续三个子项目都需要**边转发边处理**同一份流：

| 子项目 | 对流式管线的要求 |
|--------|-----------------|
| AI-1 provider 抽象 | 逐 chunk **改写**：把 Anthropic 的 `content_block_delta` 事件翻译成 OpenAI 的 `chat.completion.chunk` |
| AI-2 计量 | 逐 chunk **读取**：`usage` 藏在流的最后一个（或倒数第二个）事件里 |
| AI-4 护栏 | 逐 chunk **改写 + 提前终止**：命中护栏时截断流并注入结构化拒绝事件 |

## 现状：管线是只读 + 同步的

pingora 本身逐块转发，风险不在它。真正的阻塞点是 Edgion 自己的插件契约：

`edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs:69`

```rust
fn run_upstream_response_body_filter(
    &self,
    body: &Option<bytes::Bytes>,   // 只读引用
    end_of_stream: bool,
    session: &mut dyn PluginSession,
) -> Option<Duration>;              // 唯一返回值是限速延迟
```

三重限制，每一重都卡死一类 AI 能力：

| 限制 | 后果 |
|------|------|
| `body` 是 `&Option<Bytes>` 只读 | 无法改写 chunk → AI-1 流式翻译、AI-4 流式改写做不了 |
| trait 是**同步**的（注释明确 "sync - no async allowed"） | 无法 await → 外部 guard 调用、Redis 写入不能在此发生 |
| 返回值只有 `Option<Duration>` | 无法提前终止流、无法替换响应 → AI-4 命中护栏后没法截断 |

值得注意：代理层 `pg_upstream_response_body_filter.rs:17` 手上握的是
`body: &mut Option<Bytes>`，**可变性是在传给插件时被丢掉的**。所以扩展管线不需要改
pingora，只需要改 Edgion 自己的 trait 与调用点。

其余现状：

| 环节 | 现状 | 风险 |
|------|------|------|
| 下游压缩 | `pg_init_downstream_modules.rs`，全局 `enable_compression` 控制，**默认关闭** | 一旦开启，对 event-stream 会缓冲 |
| 缓存 | 行为待确认 | 缓存流式响应无意义且会阻塞 |
| 超时 | pingora 的 `read_timeout` / `write_timeout` 是**单次读写**超时（`proxy_h1.rs:39`、`proxy_h2.rs:171`），语义对流式正确 | 需确认上层没有叠加"整请求超时"；`idle_timeout` 需足够长 |
| 请求体缓存 | `edgion-gateway/src/request_body/`（`mod.rs` + `storage.rs`，支持内存→磁盘溢出） | 已具备；需确认对 AI 路由的默认开启策略 |
| 上游 fork 的 body replay | pingora fork 已加入 request body cache 与 `set_bodyless_request_replay_buffer`（commit `c78d9e6`、`57328f1`） | 需与 `tasks/archive/completed-tasks/TODO-body-caching.md` 对齐进度 |

未实现 pingora 的 `response_body_filter`（只有 `upstream_response_body_filter`），响应体
默认不被整体拦截 —— 对流式是好消息，不要因为本任务而引入整体拦截。

## 需要实现

### 1. 流式响应识别

- [ ] 在 `EdgionHttpContext` 增加 `is_streaming_response: bool` 标记
- [ ] 在 `pg_upstream_response_filter`（拿到上游响应头时）判定并置位，判定条件：
  - `Content-Type` 以 `text/event-stream` 开头
  - 可扩展：`application/grpc`、`application/x-ndjson`、`multipart/x-mixed-replace`
  - 无 `Content-Length` 且 `Transfer-Encoding: chunked` 可作为辅助信号
- [ ] 判定逻辑复用 / 落在 `proxy_http/content_type.rs`

### 2. 关闭会破坏流式的处理

- [ ] 压缩：`enable_compression` 开启时，对流式响应跳过（pingora 的 compression module
      支持按响应决定是否启用，需确认具体 API）
- [ ] 缓存：流式响应一律 bypass cache
- [ ] 响应 body 插件：`is_streaming_response` 为 true 时跳过会累积 body 的插件

### 3. 插件契约：流式安全声明

- [ ] 在响应 body 插件 trait 上增加能力声明，例如 `fn streaming_safe(&self) -> bool { false }`
  - 逐块处理、不累积的插件（如带宽限速）返回 `true`，正常参与流式响应
  - 默认 `false`：需要完整 body 的插件在流式响应上自动跳过，避免新插件默认踩坑
- [ ] 跳过时记录一条 debug 日志 + 一个计数指标，避免"插件静默不生效"变成排查黑洞
- [ ] 盘点现有全部 `upstream_response_body` 插件，逐个标注

### 4. 可写 + 可异步的流式管线（本任务的核心增量）

这一节是原 `mcp/01` 没有的部分，也是 AI-1/AI-2/AI-4 的共同前提。

- [ ] **设计决策**：是扩展现有 `UpstreamResponseBodyFilter` trait，还是新增一个并行的
      `StreamingResponseProcessor` trait？倾向后者 —— 现有 trait 的只读 + 同步是刻意设计
      （见其文件头关于"不做 per-chunk 日志"的说明），带宽限速类插件不该被迫承担新契约的
      复杂度。新 trait 只有显式声明的 AI 类插件才实现
- [ ] 新管线需要提供的能力：
  - 拿到 `&mut Option<Bytes>`，可替换/丢弃/追加 chunk
  - 可返回"终止流"信号，并允许写入一段收尾内容（AI-4 的结构化拒绝）
  - 支持 async（外部 guard 调用、Redis 写入）；同步实现走零成本路径
  - 跨 chunk 的**有界**状态（SSE 事件可能跨 chunk 边界断开，必须能缓存半个事件）
- [ ] **不得违反**的边界：跨 chunk 状态必须有硬上限（如单个未完成 SSE 事件 64 KiB），
      超限即放弃处理并透传，绝不无界累积 —— 这是 `CLAUDE.md` "HTTP body handling
      invariants" 第 2 条的要求
- [ ] SSE 帧解析器：从字节流中切出完整的 `event:` / `data:` / `id:` 帧，供上层按语义处理，
      而不是让每个 AI 插件各自写一遍字节扫描

### 5. 请求侧地基

- [ ] 与 `tasks/archive/completed-tasks/TODO-body-caching.md` 对齐当前落地状态
- [ ] 确认 AI 路由上默认开启请求体缓存的策略（LLM 请求体从几百字节到几 MB，
      prompt 可能很大；需要一个独立于全局的上限）
- [ ] 确认 body 超限时的行为是"放弃重试"而不是"报 413 拒绝请求"

### 6. 首字节后禁止 failover（通用边界）

一旦第一个响应字节已写给客户端，再切换上游会让客户端收到两个来源拼接的内容，比直接
失败更糟。这在流式场景尤其致命。

- [ ] 在响应第一个 body chunk 写给下游后，置位 `response_started` 标记
- [ ] `response_started == true` 时禁止任何上游重试 / failover，直接把错误透传给客户端
- [ ] 补一个指标：因响应已开始而放弃重试的次数
- [ ] 相关重试 / failover 逻辑位置：`edgion-gateway/src/lb/attempt.rs`、
      `pg_fail_to_connect.rs`、`pg_error_while_proxy.rs`

### 7. 超时

- [ ] 确认没有对整个响应设置总时长上限（若有，流式必须豁免）
- [ ] 确认长时间无数据的流不会被 idle timeout 提前断开（MCP 靠 ping 保活但间隔可能达
      分钟级；LLM 首 token 前的思考时间也可能很长）
- [ ] 若需要，为流式响应提供独立的 idle timeout 配置项

## 测试

- [ ] 后端逐秒推送 SSE 事件，验证网关侧每个事件的到达时间与后端发送时间之差在合理范围
      （而非全部堆在最后）
- [ ] `enable_compression: true` 时 SSE 仍然逐事件到达
- [ ] HTTP/1.1 chunked 与 HTTP/2 两种下游协议都覆盖
- [ ] 长空闲（超过普通 idle timeout）后仍能收到后续事件
- [ ] 配置了 body 改写类插件的路由上，SSE 请求不被破坏
- [ ] **新管线**：一个测试插件把每个 chunk 的内容改写后转发，客户端收到的是改写后的流
- [ ] **新管线**：SSE 事件被人为切在 chunk 边界中间（`data: {"a"` / `:1}\n\n`），
      帧解析器仍能还原完整事件
- [ ] **新管线**：未完成事件超过硬上限时放弃处理并透传，不 OOM
- [ ] **新管线**：插件返回终止信号后，流被截断且收尾内容被写出
- [ ] 上游连接被拒 → 重试到其它实例成功，body 完整送达
- [ ] SSE 流已推送 3 个事件后上游断开 → 客户端收到连接中断，**不发生**重试，不出现事件重复
- [ ] 大 body（超出 buffer limit）→ 不重试但请求本身仍成功

`edgion-tests/src/server/test_server.rs` 已有 streaming-body endpoints（用于 per-try-timeout
测试），可复用或扩展成 SSE 测试端点。

## 备注

这一步做完，MCP 之外的 OpenAI/Anthropic 流式代理、A2A 也同时可用了。建议独立验收，
不要和后面的 AI-1 provider 抽象捆绑。

第 4 节（可写管线）是本任务里唯一需要动核心插件契约的部分，改动面比其余各节大一个量级，
建议单独设计评审。

## Design review findings

The original task overstates the need for an asynchronous response-body hook.
The minimum useful AI Gateway foundation is a writable, bounded, synchronous
streaming pipeline:

- SSE passthrough does not require asynchronous processing.
- Provider event translation, such as Anthropic events to OpenAI events, is a
  local synchronous transformation.
- Usage extraction is a local synchronous observation. Persisting the final
  aggregate belongs in an existing asynchronous response-end or logging phase,
  not in the per-chunk path.
- Local keyword, regex, JSON-field, token-count, or finite-state guardrails can
  run synchronously.
- Asynchronous per-stream processing is required only when output must wait for
  an external online decision, such as a remote content-safety service.

Consequently, asynchronous processing is an optional extension for remote
inline inspection, not a prerequisite for AI-0, provider translation, or usage
metering.

### Pingora and Envoy boundary

Pingora deliberately exposes lifecycle callbacks similar to NGINX/OpenResty
rather than Envoy's general pause/resume filter-chain state machine. Edgion
must preserve that boundary:

- Keep the existing `UpstreamResponseBodyFilter` synchronous, read-only, and
  allocation-free for ordinary traffic, bandwidth limiting, and Wasm chunk
  observation.
- Do not add Envoy-style continuation handles, independent filter-owned
  buffering, arbitrary resume calls, or multiple independently paused filters.
- The proxy pump must remain the sole owner of upstream reads, downstream
  writes, ordering, backpressure, and termination.
- If asynchronous streaming is added later, expose one opt-in Edgion pipeline
  as a single inline await owned by the pump. Individual processors must not
  spawn per-chunk forwarding tasks or place chunks in an unbounded queue.

Pingora already runs the surrounding response filter path asynchronously and
uses a bounded task channel. An inline await does not inherently break
backpressure; spawning work, allowing concurrent chunk processing, or using an
unbounded queue does.

### Revised capability order

Implement and validate the foundation in this order:

1. Detect the final response stream protocol from an explicit normalized media
   type. Do not classify every response without `Content-Length`, or every H1
   chunked response, as streaming.
2. Fix downstream compression enablement and disable compression for streaming
   media types before the compression module enters its body phase.
3. Add a separate writable `StreamingResponseProcessor` pipeline. Do not widen
   the existing body-filter trait.
4. Add a shared bounded SSE decoder and per-request processor state.
5. Add pump-owned graceful termination that can emit a final event, end the
   downstream response, cancel the upstream response, and suppress all retry.
6. Add an asynchronous processor path only when a concrete remote inline
   inspection feature requires it.

The processor result should remain deliberately smaller than Envoy's filter
contract:

```rust
enum StreamingAction {
    Continue,
    Finish { tail: Option<bytes::Bytes> },
}
```

There is no general `StopIterationAndBuffer`, watermark API, continuation
handle, or plugin-driven resume operation.

### Bounded-state and failure rules

- An incomplete SSE event must have a hard byte limit.
- On overflow, flush the pending original bytes exactly once, permanently
  bypass semantic processing for that processor, and pass subsequent chunks
  through without parsing.
- Remote inline inspection, if implemented, must use a hard deadline, bounded
  global concurrency, at most one in-flight decision per request, and an
  explicit existing fail-open/fail-close policy.
- Do not call a remote guard or write Redis once per token or transport chunk.
  Batch remote inspection at semantic event/window boundaries. Aggregate usage
  locally and persist it once after the final usage event or stream completion.
- Per-chunk tracing and `PluginLog` entries remain forbidden. Aggregate bounded
  per-request outcomes and emit them once outside the body hot path.

### Risk assessment

| Change | Relative size | Risk |
|--------|---------------|------|
| Make the Pingora body callback async without termination semantics | Small | Low to medium |
| Add a synchronous writable Edgion streaming pipeline | Medium | Medium |
| Add bounded SSE framing and provider transformation | Medium | Medium |
| Emit a final event and immediately terminate H1/H2 upstream and downstream streams | Medium to large | Medium to high |
| Recreate an Envoy-style general pause/resume filter manager | Large | Unacceptable for this task |

The highest-risk work is not the Rust `async` keyword. It is graceful
termination across H1/H2, upstream connection reuse, H2 reset behavior,
downstream EOS/framing, cache state, retry suppression, cancellation, and
correct classification of an intentional guardrail termination versus an
upstream proxy failure.

### Current-code corrections

- `enable_compression: true` currently omits the compression module instead of
  enabling it. Streaming work must first fix and test ordinary compression,
  then selectively disable it for streaming responses.
- Edgion currently has no HTTP response-cache implementation to bypass. Record
  this as a verified non-applicable item; do not add placeholder cache logic.
- Request-body buffering, disk spill, body requirements, body mutation framing,
  and Pingora replay-buffer integration are already implemented. The old
  `tasks/archive/completed-tasks/TODO-body-caching.md` is stale. This task only needs to verify
  that replay-buffer overflow disables retry without rejecting the original
  request and decide whether AI routes need a policy override.
- Use Pingora's authoritative downstream `response_written()` state rather than
  adding a second `response_started` boolean. Once response headers are
  committed, every retry path must be suppressed.
- Pingora peer `idle_timeout` controls pooled-connection idleness, not the
  interval between streaming response events. Long silent streams are governed
  by upstream read/backend-request deadlines. Explicit route request deadlines
  and client `grpc-timeout` values must never be silently waived for SSE.

## Status 2026-08-03: request side landed, AI-0a answered, response side designed

The AI-0a investigation gate is closed by the Pingora fork itself: commit
`b1d520e` ("Add canonical request-body transport interface") plus `b829a34`
(request body cache) provide the transport boundary this task was waiting on,
and Edgion has already integrated the request side:

- `pg_request_body_filter_action.rs` drives `RequestBodyAction::{Continue,
  Terminate}`; `upstream_request_body_disposition` and `request_retry_allowed`
  are implemented on `ctx.request_body_stream_active`.
- `BodyStorage` (memory→disk spill) implements the fork's `RequestBodyBuffer`
  for retry replay; the body-gate admission model (R1–R23) is live.
- The fork also fixed no-retry-after-response-commit (section 6 of this task),
  so sections 5 and 6 are done; section 7's analysis is confirmed in the
  design-review notes above.

Remaining scope = the response side only (sections 1–4), redesigned as five
work items in `docs/superpowers/specs/2026-08-03-ai-gateway-phase1-design.md`:
streaming classification (explicit media type), compression fix + streaming
exemption, a writable sync `StreamingResponseProcessor` pipeline installed via
ctx (not a new EdgionPlugins stage), a bounded shared SSE decoder, and the
usage-extraction processor (phase-1 slice of AI-2). Stream termination
(`Finish { tail }`) is deferred to AI-4 together with fork response-side
terminate support. That spec is the implementation source of truth for phase 1
(AI-0 remainder + AI-1a).
