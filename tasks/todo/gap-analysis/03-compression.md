# 03 - Compression (响应压缩)

**优先级**: P0 - 生产必备

## 问题

Edgion 完全没有响应压缩支持。对外网流量来说，gzip/Brotli 可减少 60-80% 传输体积，是最基本的性能优化。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | gzip（via NGINX） |
| Kong | gzip（via NGINX） |
| Envoy | gzip / Brotli / Zstd，支持 per-route 选择压缩算法 |

## 需要实现

- [ ] gzip 压缩支持
- [ ] Brotli 压缩支持
- [ ] 可配置的压缩阈值（最小 body 大小）
- [ ] 按 Content-Type 过滤（只压缩 text/html、application/json 等）
- [ ] 尊重客户端 Accept-Encoding header
- [ ] 作为 HTTP 插件实现，支持 per-route 开关

## 技术选型建议

- `flate2` crate（gzip）
- `brotli` crate（Brotli）
- Pingora 层面是否已有压缩支持需调研
