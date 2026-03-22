---
name: logging-features
description: 日志体系：Access Log、SSL/TCP/TLS/UDP 协议日志、系统日志配置。
---

# 日志体系

## 日志分类

| 类型 | 用途 | 配置位置 | 默认路径 |
|------|------|---------|---------|
| 系统日志 | 控制面/数据面运行时日志 | `[logging]` | `logs/edgion-{type}.log` |
| Access Log | HTTP/gRPC 请求日志（JSON） | `[access_log]` | `logs/edgion_access.log` |
| SSL Log | HTTPS 连接日志 | `[ssl_log]` | `logs/ssl.log` |
| TCP Log | TCP 代理日志 | `[tcp_log]` | `logs/tcp_access.log` |
| TLS Log | TLS 代理日志 | `[tls_log]` | `logs/tls_access.log` |
| UDP Log | UDP 代理日志 | `[udp_log]` | `logs/udp_access.log` |

## Access Log

Access Log 是 Edgion 的核心可观测手段，每条请求生成一条 JSON 格式日志，包含足够信息定位问题。

### 关键字段

| 字段 | 说明 |
|------|------|
| `sv` | Server Version（Controller → Gateway 同步版本） |
| `request_id` | 请求唯一 ID |
| `method` | HTTP 方法 |
| `path` | 请求路径 |
| `status` | 响应状态码 |
| `duration_ms` | 请求耗时（毫秒） |
| `upstream_addr` | 后端地址 |
| `route_name` | 匹配的路由名称 |
| `stage_logs` | 插件执行日志（每个插件 ≤ 40 字节） |

### 配置示例

```toml
[access_log]
enabled = true
[access_log.output.localFile]
path = "logs/edgion_access.log"
queue_size = 80000
[access_log.output.localFile.rotation]
strategy = "daily"
max_files = 7
```

## 协议日志

SSL/TCP/TLS/UDP 日志共享相同的配置 Schema（详见 [../02-config/01-gateway-config.md](../02-config/01-gateway-config.md) 的业务日志 Schema 段落）。

### 启用/禁用

```toml
[tcp_log]
enabled = true      # 启用 TCP 日志
[tcp_log.output.localFile]
path = "logs/tcp_access.log"
```

## 系统日志

系统日志使用 Rust `tracing` 框架，支持模块级别过滤：

```toml
[logging]
log_level = "info,edgion::core::gateway::routes=debug"  # 特定模块 debug
json_format = true                                        # 结构化 JSON
```
