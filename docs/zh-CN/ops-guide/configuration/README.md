# 配置参考

Edgion 有三个独立的配置层。在查阅 schema 之前，先确定需要修改的是哪一层——它们的载体、作用域和重载语义各不相同。

## 配置层次

| 配置层 | 载体 | 作用域 | 变更方式 |
|--------|------|--------|---------|
| Controller 进程配置 | YAML 文件（`config/edgion-controller.yaml`） | 单个 Controller 实例 | 重启进程（部分段支持 `POST /api/v1/reload` 热重载） |
| Gateway 进程配置 | YAML 文件（`config/edgion-gateway.yaml`） | 单个 Gateway 实例 | 重启进程 |
| 运行时配置 | `EdgionGatewayConfig` CRD（`edgion.io/v1alpha1`） | 整个 GatewayClass | 动态生效（无需重启） |

两个进程配置都是启动时从 YAML 文件加载，可通过 `--config-file` 指定路径。CRD 是集群作用域的，通过 `GatewayClass.spec.parametersRef` 引用。

> **联邦 Center**：第四个二进制 Center 有自己的 YAML 进程配置（`config/edgion-center.yaml`，同样使用 `--config-file`），仅用于多集群联邦部署。与 Controller 不同，Center 不会自动生成管理员凭据——必须配置 `local_auth` 或 `auth`，否则业务路由以 503 失败关闭。要将 Controller 加入 Center，在 Controller 配置中添加 `center` 段（见 [controller-config.md](controller-config.md#联邦客户端-center)）。

> 四个配置层均为 **YAML** 格式。Edgion 中没有 TOML 配置。

## 我应该修改哪个配置层？

| 我需要修改… | 配置层 |
|------------|--------|
| Controller gRPC / Admin 监听地址、`conf_center` 模式、管理员认证 | Controller 配置 |
| Gateway 连接的 Controller、日志路径、Pingora worker 调优、限流精度 | Gateway 配置 |
| HTTP 超时、重试、真实 IP 提取、安全策略、全局插件 | `EdgionGatewayConfig` CRD |

## 内容

| 文档 | 说明 |
|------|------|
| [controller-config.md](controller-config.md) | Controller YAML——完整参数参考（server、conf_center、认证、联邦、CLI Token） |
| [gateway-config.md](gateway-config.md) | Gateway YAML——完整参数参考（server、日志、业务日志、限流） |
| [edgion-gateway-config.md](edgion-gateway-config.md) | EdgionGatewayConfig CRD——GatewayClass 级运行时配置 |
