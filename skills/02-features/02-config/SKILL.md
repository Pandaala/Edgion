---
name: config-reference
description: 配置 Schema 参考：Controller/Gateway TOML 配置和 EdgionGatewayConfig CRD 的完整字段定义。
---

# 02 配置参考

> 配置分三层，先确定改哪一层，再查对应 Schema。

## 配置层次

| 层 | 载体 | 作用域 | 变更频率 |
|----|------|--------|---------|
| Controller 进程配置 | TOML 文件 | 单个 Controller 实例 | 低（启动时加载） |
| Gateway 进程配置 | TOML 文件 | 单个 Gateway 实例 | 低（启动时加载） |
| 运行时配置 | EdgionGatewayConfig CRD | 所有同 GatewayClass 的 Gateway | 中（动态生效） |

## 文件清单

| 文件 | 主题 |
|------|------|
| [00-controller-config.md](00-controller-config.md) | Controller TOML 完整 Schema |
| [01-gateway-config.md](01-gateway-config.md) | Gateway TOML 完整 Schema |
| [02-edgion-gateway-config.md](02-edgion-gateway-config.md) | EdgionGatewayConfig CRD Schema |

## 快速决策：改哪一层？

| 我要改… | 改这层 |
|---------|--------|
| Controller 的 gRPC/Admin 监听地址 | Controller TOML `[server]` |
| conf_center 模式（FileSystem/K8s） | Controller TOML `[conf_center]` |
| Gateway 连接的 Controller 地址 | Gateway TOML `[gateway]` |
| 系统日志级别/格式 | Controller/Gateway TOML `[logging]` |
| Access/SSL/TCP/TLS/UDP 日志开关和路径 | Gateway TOML 各 `*_log` 段 |
| Pingora worker 线程数/连接池 | Gateway TOML `[server]` |
| 全局 HTTP 超时、重试次数 | EdgionGatewayConfig CRD `spec.httpTimeout` / `spec.maxRetries` |
| 全局真实 IP 提取 | EdgionGatewayConfig CRD `spec.realIp` |
| 全局安全防护（XFF 限制、SNI 匹配） | EdgionGatewayConfig CRD `spec.securityProtect` |
| 全局插件 | EdgionGatewayConfig CRD `spec.globalPluginsRef` |
