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

仓库提供了一键部署脚本，可以安装 CRD、Controller（控制平面）、Gateway（数据平面）及基础配置：

```bash
cicd/deploy/kubernetes/scripts/deploy.sh -y
```

如果只需要先安装 CRD，运行：

```bash
cicd/deploy/kubernetes/scripts/install_crds.sh
```

部署前建议先确认：

- 集群里已经安装或可安装 Gateway API CRD
- 你有 `kubectl` 和目标命名空间的操作权限
- 需要的镜像、RBAC 和环境配置已按部署文档准备

下一步建议阅读：

- [运维指南 / Gateway 总览](../ops-guide/gateway/overview.md)
- [运维指南 / GatewayClass 配置](../ops-guide/gateway/gateway-class.md)
- [用户指南 / HTTPRoute 总览](../user-guide/http-route/overview.md)

## Standalone 快速入口

仓库提供了启动脚本，以本地进程方式启动 Controller 和 Gateway，并加载内置的文件系统配置：

```bash
cicd/deploy/standalone/start.sh
```

默认使用 `/usr/local/edgion` 作为工作目录；可以通过 `--work-dir <path>` 指定，通过 `--foreground` 将两个进程保持在终端前台运行。

这条路径更适合：

- 本地开发和排障
- 在文件系统模式下直接加载 YAML
- 不依赖 Kubernetes API 的单机环境

如果你需要理解进程级配置和运行目录，继续看：

- [开发指南 / Work Directory 设计](../dev-guide/work-directory.md)

## 配置文件与示例入口

仓库里已经有几类可以直接参考的内容：

- `config/edgion-controller.yaml`
- `config/edgion-gateway.yaml`
- `edgion-tests/k8s/conf/`
- `edgion-tests/integration/conf/`

理解方式建议是：

- Kubernetes 侧看 `edgion-tests/k8s/conf/`
- 本地集成测试和文件系统模式看 `edgion-tests/integration/conf/`
- 进程级配置看 `config/*.yaml`

## Admin API 认证

Controller（控制平面）和 Center 的 Admin API 均**强制要求认证**，所有 `/api/v1/*` 业务路由都需要有效凭证。

### Controller

Controller Admin API 采用**纯 Token**（Istio 风格）认证。预共享管理员 Token 必须由运维人员显式提供——Controller 不再自动生成。
Kubernetes 推荐方式是运行 `deploy.sh`，该脚本会幂等地创建 `edgion-admin-credentials` Secret（单个 `token` 键）
并将其作为 `EDGION_ADMIN_TOKEN` 环境变量注入到 controller Deployment。

运行 `deploy.sh` 后读取 Token：

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

然后用它授权 CLI：
```bash
edgion-cli login --token <TOKEN>
```

若未配置管理员 Token（且未配置 `cli_tokens`），服务器仍可启动，但所有 Admin API 业务路由返回 **503 fail-close**，直到提供 Token 为止。
完整的 Token 引导选项参见 [Controller 配置](../ops-guide/configuration/controller-config.md#authentication)。

### Center

Center **不支持**自动生成密码。运维人员必须在 Center 配置中提供 `local_auth` 或 `auth`。若两者均未配置：

- `/health`、`/ready`、`/metrics` 和 `/api/v1/auth/status` 仍可访问。
- 所有其他业务路由返回 **503 fail-close**，直到配置凭证为止。

设置 `enabled: false` 会被**忽略**并记录 WARN 日志——认证不可关闭。

完整配置详情参见 [Controller 配置参考](../ops-guide/configuration/controller-config.md#authentication)。

## 接下来建议做什么

如果你已经完成部署，下一步直接读：

- [第一个 Gateway](./first-gateway.md)

如果你还不清楚 Gateway API 里的对象关系，先读：

- [核心概念](./core-concepts.md)
