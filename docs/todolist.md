

🔥 绝大部分插件都在 request_filter

鉴权

路由逻辑

限流

黑白名单

访问日志

上游选择（LB）

HTTP 属性修改

🟨 早期 early_request_filter 适用场景

特殊路由重写

建立 request_id

X-Forwarded-* 处理

CORS 预检 OPTIONS

请求过大 / 协议预检查

仅 ~5% 插件会用到。

🟨 response_filter 适用场景（少）

gzip

header modification

error transform





Gateway API 官方已经确认未来也不打算做“通用 Filter”

特别是以下类型：

Auth（JWT, OAuth, API-Key）

Security（WAF, ACL, IP 限制）

RateLimit

Request/Response rewrite

Header manipulation

Body filter

Load balancing logic

Circuit break

Retry

Compression

Logging / Tracing 插件

官方给出的理由：

不同控制器对这些功能的实现差异太大，无法标准化。

这和你现在做的情况很像：
每个网关都是自己设计一套 Filter 框架。