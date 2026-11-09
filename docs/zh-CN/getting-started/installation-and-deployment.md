# 安装部署

本页只回答一个问题：第一次接触 Edgion 时，应该从哪条部署路径开始。

## 先选部署模式

Gateway 的重试预算记账要求 CPU 支持无锁的 128 位原子操作。amd64 CPU 必须支持
CMPXCHG16B，虚拟机也必须向来宾系统暴露该能力；受支持的 arm64 主机使用原生原子实现。
Gateway 会在启动服务前检查，不满足时明确报错退出，不回退到锁实现。
即使当前没有配置重试预算也需要满足此要求，因为策略可以通过热更新添加。
Controller 和 edgion-cli 不受此要求影响。

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

从源码构建这三个二进制时，请通过 `allocator-jemalloc`（默认）、
`allocator-mimalloc` 或 `allocator-system` 选择分配器。依赖 crate 名称
`tikv-jemallocator` 和 `mimalloc` 不是这些二进制的 Cargo feature 名称。

Gateway 还提供可选的子系统 feature：`kafka`、`otlp`、`geoip` 和 `wasm`。
聚合 feature `full` 会开启全部子系统，并且包含在默认 feature 中。使用
`--no-default-features` 加显式 feature 列表构建时，需要加上 `full`（或所需的单个子系统），
否则这些子系统不会被编译进二进制。被编译掉的子系统不会 fail-open：主日志输出配置为 Kafka/OTLP，或 SSL 日志输出配置为 OTLP 时
Gateway 会启动失败（备用输出会被跳过并打印告警），`LdapAuth` 与 `Wasm` 插件条目会被拒绝，GeoIP
查询会报告数据库不可用，由各个 `GeoIpLocation` 插件的 `failOpen` 配置决定处理方式。
`edgion-gateway --print-build-capabilities` 会列出二进制中包含哪些子系统。

### 用容器镜像运行非 Kubernetes 模式

上面的 `start.sh` 路径使用发行版二进制，CRD schema 已经在磁盘的 `config/crd` 下，无需额外处理。

但如果你改用**容器镜像**运行 controller 的非 Kubernetes 配置中心模式（`conf_center.type: file_system`
或 `etcd`），要注意：runtime 镜像**不打包** CRD schema——它只在非 Kubernetes 模式下需要，而 Kubernetes
模式的 schema 来自 API server。缺少它时 controller 会在启动阶段以 `FATAL: No schemas loaded in non-K8s
mode` 退出。请把 CRD 目录挂载到 `<work_dir>/config/crd`，例如：

```bash
docker run \
  -v "$PWD/config/crd:/usr/local/edgion/config/crd:ro" \
  -v "$PWD/edgion-controller.yaml:/usr/local/edgion/edgion-controller.yaml:ro" \
  docker.io/pandaala/edgion-controller:<tag> \
  --config-file /usr/local/edgion/edgion-controller.yaml
```

Gateway 镜像从不加载 CRD schema，因此这一点只涉及 controller。

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
