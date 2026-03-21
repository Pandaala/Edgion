---
name: gateway-http-route-engine
description: HTTP 路由匹配引擎：RadixPath、RegexSet、ConfHandler、代理实现、LB Policy 同步。
---

# HTTP 路由引擎

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### 模块结构

<!-- TODO:
routes/http/
├── conf_handler_impl.rs    # HTTPRoute 更新处理
├── match_engine/
│   ├── radix_path/         # Radix tree 路径匹配
│   ├── radix_route_match/  # Radix 路由匹配
│   └── regex_routes_engine/ # Regex 路由匹配
├── proxy_http/             # Pingora ProxyHttp trait 实现
├── routes_mgr/             # 路由管理器（per-domain）
├── match_unit/             # 路由匹配信息结构
├── lb_policy_sync/         # LB 策略同步
├── redirect_http/          # HTTP 重定向处理
└── tests/
-->

### Radix Path 匹配

<!-- TODO: Radix tree 实现、Exact/Prefix 支持、优先级权重计算 -->

### Regex 路由匹配

<!-- TODO: RegexSet 批量匹配、性能考虑 -->

### ConfHandler 处理

<!-- TODO: HTTPRoute 资源变更时的处理流程 -->

### 代理实现

<!-- TODO: Pingora ProxyHttp trait 的具体实现 -->

### 路由管理器

<!-- TODO: per-domain 路由规则管理、DomainRouteRules 构建 -->
