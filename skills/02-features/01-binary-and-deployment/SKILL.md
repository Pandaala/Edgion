---
name: binary-and-deployment
description: 三个 bin（edgion-controller/gateway/ctl）的启动方式、CLI 参数、部署模式、Feature Flags。
---

# 01 二进制与部署

> Edgion 是单 Crate、三 bin 的项目。本节描述各 bin 的启动方式、CLI 参数和常见部署模式。

## 文件清单

| 文件 | 主题 |
|------|------|
| [00-binaries.md](00-binaries.md) | 三个 bin 的入口、CLI 参数完整参考、work_dir 优先级 |
| [01-deployment-patterns.md](01-deployment-patterns.md) | 常见部署方式：FileSystem 开发模式、K8s 生产模式、HA 部署 |
| [02-feature-flags.md](02-feature-flags.md) | Cargo Feature Flags 矩阵：allocator / TLS backend / 测试选项 |
| [references/](references/) | Feature Flags 详细矩阵 |
