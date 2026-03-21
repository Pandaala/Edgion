---
name: gateway-pingora-lifecycle
description: Pingora ProxyHttp 生命周期：7 阶段回调、ConnectionFilter、EdgionHttpContext 状态传递。
---

# Pingora ProxyHttp 生命周期

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/03-data-plane.md`（生命周期部分）

## 待填充内容

### ProxyHttp 回调链

<!-- TODO:
1. early_request_filter()   — ACME、超时、keepalive
2. request_filter()         — 元数据提取、路由匹配、RequestFilter 插件
3. upstream_peer()          — 后端选择、LB、超时配置
4. connected_to_upstream()  — 连接回调
5. upstream_response_filter() — 响应插件、Headers
6. upstream_response_body_filter() — 分块带宽限制
7. response_filter()        — 异步响应处理
8. logging()                — Metrics + AccessLog
-->

### ConnectionFilter（TCP 层）

<!-- TODO:
- StreamPlugins 在 TLS/HTTP 之前的原始 TCP 层执行
- IP 限制、TLS 路由选择
-->

### EdgionHttpContext

<!-- TODO:
- 每请求状态载体，贯穿整个 HTTP 生命周期
- 零拷贝设计
- 包含的关键字段
-->

### 错误响应

<!-- TODO: 400/404/421/500/503 错误响应生成 -->

### Server Header

<!-- TODO: Server header 配置 -->
