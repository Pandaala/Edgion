# AI-6/03 - JSON-RPC 协议感知：指标与限流

**优先级**: P2
**依赖**: AI-0；本目录 01；05 的重试扩展点；限流核心与 [AI-2](../02-metering-and-cost.md) 共用
**预估**: 大（本支线成本最高的一项）
**来源**: 原 `mcp/05-jsonrpc-observability-ratelimit.md`

## 问题

其余任务都把 MCP 流量当作"带 session 的流式 HTTP"处理，不解析内容。要做以下事情就必须
**真正解析 JSON-RPC body**：

- 知道每个请求调用的是 `tools/call` 还是 `tools/list`
- 按 tool 名做限流（防止某个昂贵 tool 被刷）
- 按 session 做配额
- method 级的错误率 / 延迟指标

**这是本支线里唯一需要"网关懂 MCP"的部分，也是成本最高的部分。启动前先确认业务确实需要，
不要为了完整性而做。**

## 成本提示

解析请求 body 意味着：

- 必须缓存完整请求 body 才能解析（复用 AI-0 的请求体缓存）
- 每个请求增加一次 JSON 解析开销
- 批量请求（JSON-RPC batch，一个 body 里多个调用）让"一请求一 method"的假设不成立
- 响应侧若也要解析（拿错误码），流式响应下要边解析 SSE 边解析 JSON，复杂度陡增
  —— 这里应复用 AI-0 的 SSE 帧解析器，不要再写一遍

因此**默认关闭，按路由显式开启**。

## MCP 的主要 method

| method | 说明 | 典型频率 |
|--------|------|---------|
| `initialize` | 建立会话、能力协商 | 每会话 1 次 |
| `tools/list` | 列出可用 tool | 低 |
| `tools/call` | **调用 tool，真正消耗资源的操作** | 高 |
| `resources/list` / `resources/read` | 资源访问 | 中 |
| `prompts/list` / `prompts/get` | prompt 模板 | 低 |
| `ping` | 保活 | 高频、需从指标中排除 |
| `notifications/*` | 通知（无响应） | 中 |

## 需要实现

### 1. 请求解析

- [ ] 只解析 `Content-Type: application/json` 的 POST body
- [ ] 只提取所需字段：`method`、`id`、`params.name`（`tools/call` 的 tool 名），
      **不做完整反序列化**
- [ ] 设置 body 解析大小上限（如 1MB），超限则跳过解析而不是拒绝请求
- [ ] 处理 JSON-RPC batch：body 是数组时提取全部 method，指标按条计数
- [ ] 解析失败不影响转发，只是丢失该请求的协议维度指标

### 2. 指标

- [ ] `mcp_requests_total{method, tool, status}`
- [ ] `mcp_request_duration_seconds{method, tool}`
- [ ] `mcp_sessions_active`（按 `Mcp-Session-Id` 去重，需要 TTL 淘汰）
- [ ] `ping` 与 `notifications/*` 单独统计或排除，避免淹没有效指标
- [ ] **基数控制**：tool 名由后端定义、可能无界，必须有 allowlist 或 top-N 之外归为
      `other`，否则会打爆时序库。新增指标须同步更新指标清单文档，否则
      `cicd/checks/check_metrics_inventory.py` 会失败

### 3. 限流

- [ ] 复用现有 `rate_limit_redis` 插件的算法实现
      （`edgion-gateway/src/plugins/common/redis_rate_limiter`）
- [ ] AI-2 已经为该核心加入"动态成本扣减"能力；本任务的 tool 级限流可以直接复用
      同一扩展，不要再改一次核心
- [ ] 限流维度：`Mcp-Session-Id` / 认证主体（来自 [02](02-oauth-resource-server.md)）/
      tool 名 / method
- [ ] 支持组合规则，如"某 session 对 `tools/call` 每分钟 60 次，其中 `expensive_tool`
      每分钟 5 次"
- [ ] 超限响应用 JSON-RPC error 格式返回，而不是裸 HTTP 429 —— MCP client 才能正确解析

```json
{"jsonrpc":"2.0","id":<原 id>,"error":{"code":-32000,"message":"rate limit exceeded"}}
```

这和 [AI-4](../04-guardrails.md) 的"结构化拒绝响应"是同一个思路：调用方是程序，
要给它能解析的拒绝，而不是传输层错误。

### 4. 与重试语义的联动

- [ ] 有了 method 解析后，把 [05-retry-semantics.md](05-retry-semantics.md) 的重试策略
      从"仅连接失败重试"升级为 method 级：`tools/list` / `ping` 等只读 method
      可放宽重试条件

## 配置形态草案

```yaml
- type: McpGovernance
  config:
    parseRequestBody: true
    maxParseBytes: 1048576
    metrics:
      enabled: true
      toolAllowlist: ["search", "read_file", "run_query"]   # 其余归为 other
    rateLimit:
      - dimension: session
        method: "tools/call"
        limit: 60
        window: 60s
      - dimension: session
        tool: "expensive_tool"
        limit: 5
        window: 60s
```

## 测试

- [ ] 各 method 的指标 label 正确
- [ ] JSON-RPC batch 请求按条计数
- [ ] 未在 allowlist 的 tool 名归入 `other`，指标基数不增长
- [ ] 超限返回 JSON-RPC error 格式且 `id` 与请求一致
- [ ] 解析开关关闭时无额外开销（对比压测）
- [ ] 畸形 JSON / 超大 body 不导致请求失败
