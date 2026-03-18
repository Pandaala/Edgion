# 安装部署

本页只回答一个问题：第一次接触 Edgion 时，应该从哪条部署路径开始。

## 先选部署模式

Edgion 当前有两条主路径：

1. **Kubernetes 模式**
   适合已经在集群里使用 Gateway API、希望通过 CRD 和控制器统一管理配置的场景。
2. **Standalone 模式**
   适合本地开发、单机调试、裸机或 VM 环境，使用本地配置文件和进程方式启动。

如果你只是想先跑起来看效果，通常建议：

- 已有 Kubernetes 集群：从 Kubernetes 模式开始
- 想本地调试代码或快速验证配置：从 Standalone 模式开始

## Kubernetes 快速入口

仓库根 README 当前给出的最短入口是：

```bash
deploy/kubernetes/scripts/deploy.sh -y
```

这条路径会安装 CRD、controller、gateway 以及基础配置。

部署前建议先确认：

- 集群里已经安装或可安装 Gateway API CRD
- 你有 `kubectl` 和目标命名空间的操作权限
- 需要的镜像、RBAC 和环境配置已按部署文档准备

下一步建议阅读：

- [运维指南 / Gateway 总览](../ops-guide/gateway/overview.md)
- [运维指南 / GatewayClass 配置](../ops-guide/gateway/gateway-class.md)
- [用户指南 / HTTPRoute 总览](../user-guide/http-route/overview.md)

## Standalone 快速入口

仓库根 README 当前给出的最短入口是：

```bash
deploy/standalone/start.sh
```

这条路径更适合：

- 本地开发和排障
- 在文件系统模式下直接加载 YAML
- 不依赖 Kubernetes API 的单机环境

如果你需要理解配置文件和运行目录，继续看：

- [开发指南 / work-directory](../dev-guide/work-directory.md)

## 配置文件与示例入口

仓库里已经有几类可以直接参考的内容：

- `config/edgion-controller.toml`
- `config/edgion-gateway.toml`
- `examples/k8stest/conf/`
- `examples/test/conf/`

理解方式建议是：

- Kubernetes 侧看 `examples/k8stest/conf/`
- 本地集成测试和文件系统模式看 `examples/test/conf/`
- 进程级配置看 `config/*.toml`

## 接下来建议做什么

如果你已经完成部署，下一步直接读：

- [第一个 Gateway](./first-gateway.md)

如果你还不清楚 Gateway API 里的对象关系，先读：

- [核心概念](./core-concepts.md)
