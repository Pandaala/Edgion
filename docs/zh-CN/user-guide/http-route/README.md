# HTTPRoute 用户指南

HTTPRoute 定义 HTTP/HTTPS 流量的路由规则，包括匹配条件、过滤器/插件、后端配置和韧性策略。

## 内容

| 文档 | 说明 |
|------|------|
| [overview.md](overview.md) | HTTPRoute 总览与核心概念 |
| [delegation.md](delegation.md) | 多租户路由委派（父 → 子 HTTPRoute） |
| [lb-algorithms.md](lb-algorithms.md) | 负载均衡算法配置 |
| [matches/](matches/README.md) | 匹配规则（路径、请求头、查询参数、HTTP 方法） |
| [filters/](filters/README.md) | 过滤器与 Edgion 插件 |
| [backends/](backends/README.md) | 后端配置（Service 引用、权重、TLS、健康检查） |
| [resilience/](resilience/README.md) | 韧性策略（超时、重试、会话保持） |
