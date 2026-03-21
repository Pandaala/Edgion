---
name: routes-features
description: 路由资源功能与 Schema：HTTPRoute、GRPCRoute、TCPRoute、TLSRoute、UDPRoute，基于 Gateway API v1.4.0。
---

# 04 路由资源功能

> 所有路由资源的完整 Schema 和功能说明，基于 Gateway API v1.4.0。

## 文件清单

| 文件 | API Version | 状态 |
|------|-------------|------|
| [00-common-concepts.md](00-common-concepts.md) | — | 路由通用概念：parentRef、backendRef、resolved_ports |
| [01-httproute.md](01-httproute.md) | `v1` | Core |
| [02-grpcroute.md](02-grpcroute.md) | `v1` | Core |
| [03-tcproute.md](03-tcproute.md) | `v1alpha2` | Experimental |
| [04-tlsroute.md](04-tlsroute.md) | `v1alpha2` | Experimental |
| [05-udproute.md](05-udproute.md) | `v1alpha2` | Experimental |

## 路由对比

| 路由 | 协议 | 匹配维度 | 过滤器 | 会话保持 | 超时/重试 |
|------|------|---------|--------|---------|----------|
| HTTPRoute | HTTP/HTTPS | path + headers + query + method | 6 种 | Cookie/Header | ✅ |
| GRPCRoute | gRPC/gRPC-Web | service + method + headers | 4 种 | Cookie/Header | ✅ |
| TCPRoute | TCP | 仅 parentRef 端口 | — | — | — |
| TLSRoute | TLS | SNI hostname | — | — | — |
| UDPRoute | UDP | 仅 parentRef 端口 | — | — | — |
