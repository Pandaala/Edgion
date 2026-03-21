---
name: ctl-architecture
description: edgion-ctl CLI 工具架构：三种 Target 模式、子命令、输出格式、HTTP 客户端。
---

# 04 Ctl 架构（edgion-ctl）

> edgion-ctl 是 Edgion 的命令行管理工具，通过 HTTP API 与 Controller/Gateway 交互。
> 支持三种 Target 模式，可对比 center/server/client 三级缓存排查同步问题。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-overview.md](00-overview.md) | 总览 + Target 模式 + API 路由 | 了解 ctl 设计和调试方法 |
| [01-commands.md](01-commands.md) | 子命令详解 | 使用/扩展 ctl 命令 |

## 快速参考

```bash
# 查看 center（ConfCenter API）中的所有 HTTPRoute
edgion-ctl get httproute

# 查看 server（Controller 缓存）中的 HTTPRoute
edgion-ctl -t server get httproute

# 查看 client（Gateway 缓存）中的 HTTPRoute
edgion-ctl -t client get httproute

# 对比 center 和 client，排查同步问题
edgion-ctl get httproute -o json > center.json
edgion-ctl -t client get httproute -o json > client.json
diff center.json client.json

# 应用配置
edgion-ctl apply -f config.yaml
edgion-ctl apply -f config-dir/

# 重载所有资源
edgion-ctl reload
```
