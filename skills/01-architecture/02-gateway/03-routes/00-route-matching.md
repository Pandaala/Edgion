---
name: gateway-route-matching
description: 路由匹配总览：多级流水线（Port→Domain→Path→DeepMatch）、per-port 隔离、注册流程、原子切换。
---

# 路由匹配总览

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/04-route-matching.md`

## 待填充内容

### 多级匹配流水线

<!-- TODO:
Port → Domain (精确/通配/catch-all) → Path (regex/radix) → Deep match (method/headers/query/Gateway 约束)
-->

### Per-port 隔离

<!-- TODO: 每个 listener 有独立路由表，通过 GlobalHttpRouteManagers -->

### Domain 匹配

<!-- TODO: 精确映射 (O(1)) + RadixHostMatchEngine (O(log n)) -->

### Path 匹配

<!-- TODO: Radix tree (Exact/Prefix) + RegexSet (RegularExpression) -->

### RadixPath 优先级

<!-- TODO: 权重 = base(exact=2000, prefix=1000) + segments*10 + (has_params? 0:5) -->

### Deep match

<!-- TODO: Gateway/Listener 约束检查、HTTP method、headers、query params -->

### 路由注册流程

<!-- TODO: HTTPRoute → cached → bucket by port → build DomainRouteRules → atomic swap via ArcSwap -->

### 多 Gateway 端口共享

<!-- TODO: 相同端口的多 Gateway 路由合并 -->
