---
name: gateway-load-balancing
description: 负载均衡策略：RoundRobin、Random、EWMA、LeastConn、ConsistentHash、WeightedSelector、健康检查集成。
---

# 负载均衡

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/06-load-balancing.md`

## 待填充内容

### 支持的策略

<!-- TODO: RoundRobin, Random, EWMA, LeastConn, ConsistentHash, WeightedSelector -->

### 后端选择流程

<!-- TODO: Route → BackendRef → 解析 Service → 获取 backends → 应用 LB 策略 → 健康检查过滤 → 返回 HttpPeer -->

### 健康检查集成

<!-- TODO: 基于 Pingora Backend 健康状态，自动排除不健康后端，恢复后自动纳入 -->

### LB 配置方式

<!-- TODO: 通过 HTTPRoute 的 ExtensionRef filter（如 LoadBalancer kind） -->

### 各算法详解

<!-- TODO:
- RoundRobin: 加权轮询
- EWMA: 指数加权移动平均，基于响应时间
- LeastConn: 最少连接数 + 定期清理
- ConsistentHash: 一致性哈希
- WeightedSelector: 加权选择器
-->
