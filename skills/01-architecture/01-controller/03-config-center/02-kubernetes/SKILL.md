---
name: kubernetes-config-center
description: KubernetesCenter 实现：K8s Reflector 监听、Leader 选举、HA 模式、ResourceController 生命周期、Status 回写。
---

# Kubernetes 配置中心

> KubernetesCenter 是生产环境的配置中心实现，通过 K8s API 监听资源变更。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-lifecycle.md](00-lifecycle.md) | 启动流程与 Leader 选举 | 调试 Controller 启动、理解 HA 行为 |
| [01-ha-mode.md](01-ha-mode.md) | HA 模式详解 | 配置高可用、理解失败转移 |
| [02-resource-controller.md](02-resource-controller.md) | ResourceController 生命周期 | 理解单资源处理流程、Status 回写 |
