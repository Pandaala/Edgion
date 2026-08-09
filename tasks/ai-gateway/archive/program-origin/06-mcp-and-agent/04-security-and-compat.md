# AI-6/04 - 安全加固与旧版传输兼容

**优先级**: P2（但 Origin 校验与 CORS 两项建议提前）
**依赖**: AI-0；本目录 01
**来源**: 原 `mcp/06-security-and-compat.md`

## 问题

两类收尾工作：

1. **安全**：MCP 规范明确要求的 Origin 校验（防 DNS rebinding），以及浏览器端
   MCP client 需要的 CORS 处理。
2. **兼容**：协议版本协商，以及仍在使用的旧版 HTTP+SSE 双端点传输。

## 一、Origin 校验（规范要求）

MCP 规范要求 server **必须校验 `Origin` 头**，防止 DNS rebinding 攻击 —— 攻击者诱导
用户浏览器访问恶意页面，该页面通过 DNS 重绑定向本地 / 内网的 MCP server 发请求。

对本地 MCP server 这是刚需；对网关后的远程 server，网关做这层校验能统一保护所有后端。

- [ ] 支持配置 `allowedOrigins` 列表（精确匹配 + 通配子域）
- [ ] `Origin` 不在列表内 → 403，不转发到上游
- [ ] 无 `Origin` 头的请求（非浏览器发起，如 CLI / SDK）默认放行，可配置为拒绝
- [ ] 与已有的 CORS 处理逻辑对齐（`proxy_http/preflight_handler.rs` 已有 preflight 处理，
      需确认是否复用；另有 `plugins/http/cors/` 插件，两者边界要先理清）

## 二、CORS

- [ ] 确认 preflight（OPTIONS）对 MCP 端点的处理正确
- [ ] `Access-Control-Expose-Headers` 必须暴露 `Mcp-Session-Id` —— 否则浏览器端 client
      读不到 session id，整个流程断在第一步
- [ ] `Access-Control-Allow-Headers` 需包含 `Mcp-Session-Id`、`MCP-Protocol-Version`、
      `Last-Event-ID`

## 三、协议版本协商

- [ ] 透传 `MCP-Protocol-Version` 请求头（确认不被现有 header 过滤逻辑丢弃）
- [ ] 可选：网关侧配置支持的版本范围，不支持的版本直接返回错误而非透传到上游
- [ ] 记录版本分布指标，用于判断何时可以下线旧版支持

## 四、旧版 HTTP+SSE 传输兼容

2024-11-05 规范的双端点传输：

```
GET  /sse       → 建立 SSE 流，服务端首个事件下发 POST 端点 URL（含 sessionId）
POST /messages?sessionId=xxx  → 发送 JSON-RPC 消息
```

网关侧的差异点：

- [ ] session 标识在 **query param** 而非 header → 会话亲和用
      `hashOn: QueryParam, key: sessionId`（[01-session-routing.md](01-session-routing.md)
      的模式 A 已覆盖能力，这里补文档与示例）
- [ ] `GET /sse` 是长连接，`POST /messages` 是短请求，两者必须路由到**同一实例** ——
      但它们的 session 标识来源不同（GET 时还没有 sessionId）。这是旧传输的固有缺陷，
      也是模式 B（session-id 编码）在旧传输上用不了的原因
- [ ] 明确支持策略：文档中标注为"兼容模式，建议后端升级到 Streamable HTTP"，
      不为它做特殊开发
- [ ] 若后端确实需要，最直接的方案是该后端只部署单实例，或用 `SourceIp` 哈希兜底

## 五、Last-Event-ID 续传

- [ ] 确认 `Last-Event-ID` 请求头被正常透传到上游
- [ ] 重连请求同样带 `Mcp-Session-Id`，会话路由保证落到同一实例（否则续传无意义）

## 测试

- [ ] 恶意 Origin 被 403 拦截
- [ ] 无 Origin 的 CLI 请求正常通过
- [ ] 浏览器场景下 `Mcp-Session-Id` 可被 JS 读取（Expose-Headers 生效）
- [ ] `MCP-Protocol-Version`、`Last-Event-ID` 完整透传
- [ ] 旧版双端点流程端到端可用（单实例后端）

## 备注

Origin 校验和 CORS header 两项建议提前到 01 一起做 —— 改动很小，但缺了会直接导致
浏览器端 MCP client 完全不可用，属于"不做就是坏的"那一类。其余部分按需推进。
