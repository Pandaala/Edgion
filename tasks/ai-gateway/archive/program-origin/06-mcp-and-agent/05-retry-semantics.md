# AI-6/05 - MCP 重试语义与亲和交互

**优先级**: P1
**依赖**: AI-0（body replay、首字节后禁 failover）；本目录 01（会话路由）
**预估**: 小
**来源**: 原 `mcp/03-retry-and-failover-boundary.md` 的 MCP 特有部分。
通用部分（请求体缓存、首字节后禁止 failover）已上移到
[../00-streaming-foundation.md](../00-streaming-foundation.md)

## 前置：已在 AI-0 解决的部分

本任务**不**重复实现以下内容，它们是通用地基：

- 请求 body 缓存与 replay（`edgion-gateway/src/request_body/`，
  上游 pingora fork 的 `set_bodyless_request_replay_buffer`）
- `response_started` 标记与"首字节后禁止任何 failover"
- body 超限时"放弃重试"而非"报 413 拒绝"的行为约定

本任务只处理 MCP 协议特有的语义。

## 一、JSON-RPC 请求体规模确认

- [ ] 确认 JSON-RPC POST（典型 body 几百字节到几十 KB）在 AI-0 设定的默认 buffer
      limit 内，不会意外落到"放弃重试"分支
- [ ] `tools/call` 传大参数（如整份文件内容）的场景要单独评估

## 二、method 的幂等语义

MCP 的 JSON-RPC method 幂等性不一致：

| method | 幂等 | 可否重试 |
|--------|------|---------|
| `initialize` | 否（会创建新 session） | 连接失败时可重试（旧 session 未建立） |
| `tools/list`、`resources/list`、`prompts/list` | 是 | 可 |
| `tools/call` | **取决于具体 tool** | 谨慎 |
| `ping` | 是 | 可 |

- [ ] **首版策略**：只在"连接建立失败 / 未收到任何响应字节"时重试，不做 method 级判断。
      这是安全且不需要解析 body 的做法
- [ ] method 级的精细重试策略留到 [03-jsonrpc-governance.md](03-jsonrpc-governance.md)
      （那时才有 JSON-RPC 解析能力），本任务只留出扩展点

## 三、与会话路由的交互（本任务的核心）

会话路由与 failover 在语义上是冲突的：目标实例挂掉时请求会落到其它实例，
而该实例上没有这个 session，必然返回"会话不存在"。在多个实例间反复重试只会
把一个明确的失败变成一串难以理解的失败。

- [ ] 明确语义：**这种情况应该快速失败并让客户端重新 `initialize`**，
      而不是无意义地在多个实例间重试
- [ ] 实现：请求带 `Mcp-Session-Id` 且路由目标不健康时，直接返回明确错误
      （建议 JSON-RPC error + 语义清晰的 message），而非 failover。可作为配置项，
      默认为快速失败
- [ ] 会话路由的模式 B（session-id 编码）下这一点更明确 —— 网关**确切知道**目标实例，
      所以"目标不可用"是一个确定结论而不是推测，错误信息可以更准确
- [ ] 补指标：因会话目标不可用而快速失败的次数（区别于普通 5xx，这类失败意味着
      客户端需要重建会话）

## 测试

- [ ] 上游连接被拒 → 重试到其它实例成功，body 完整送达（`initialize` 场景）
- [ ] 带 session id 的请求，目标实例不健康 → 快速失败，**不**在其它实例间重试
- [ ] 快速失败的响应体是 JSON-RPC error 格式，客户端 SDK 能解析并触发重新 initialize
- [ ] 配置项关闭快速失败时，行为回到普通 failover（并验证确实会得到"会话不存在"）
