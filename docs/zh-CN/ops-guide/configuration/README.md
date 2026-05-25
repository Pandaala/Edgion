# 配置参考

Edgion 采用三层配置体系，覆盖进程级、GatewayClass 级两个维度。

## 配置层次

| 层 | 载体 | 作用域 | 变更方式 |
|----|------|--------|---------|
| Controller 进程配置 | TOML 文件 | 单个 Controller 实例 | 重启进程 |
| Gateway 进程配置 | YAML 文件 | 单个 Gateway 实例 | 重启进程 |
| 运行时配置 | EdgionGatewayConfig CRD | 整个 GatewayClass | 动态生效 |

## 内容

| 文档 | 说明 |
|------|------|
| [controller-config.md](controller-config.md) | Controller TOML 完整配置参数 |
| [gateway-config.md](gateway-config.md) | Gateway YAML 完整配置参数 |
| [edgion-gateway-config.md](edgion-gateway-config.md) | EdgionGatewayConfig CRD — GatewayClass 级运行时配置 |
