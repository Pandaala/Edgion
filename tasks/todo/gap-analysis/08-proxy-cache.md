# 08 - Proxy Cache (响应缓存)

**优先级**: P1 - 竞品标配
**状态**: 已在 roadmap 中，建议提前

## 问题

网关层缓存可显著降低后端压力，对 GET 请求的重复响应直接从缓存返回。Edgion roadmap 中已有此项，但三大竞品均已实现，建议提高优先级。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `proxy-cache` 插件，支持内存和磁盘缓存 |
| Kong | Proxy Caching 插件（支持内存/Redis），Proxy Caching Advanced（Enterprise） |
| Envoy | HTTP Cache Filter（实验性），基于 RFC 7234 |

## 需要实现

- [ ] 内存缓存（LRU 淘汰策略）
- [ ] 缓存 key 生成策略（method + host + path + 可配置的 query/header）
- [ ] 遵循 HTTP 缓存语义（Cache-Control、Expires、ETag、If-None-Match）
- [ ] 可配置的缓存条件（状态码白名单、Content-Type、最大 body 大小）
- [ ] TTL 配置（默认 TTL + 遵循响应头 TTL）
- [ ] 缓存清除接口（PURGE 方法或管理 API）
- [ ] 可选的 Redis 后端（分布式缓存）
- [ ] 作为 HTTP 插件实现
