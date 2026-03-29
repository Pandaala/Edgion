# Conformance Scripts

本目录包含 Gateway API conformance 测试的全套操作脚本。

## 脚本概览

| 脚本 | 用途 | 典型场景 |
|------|------|----------|
| `setup-orbstack.sh` | OrbStack 专用：本地构建镜像 + 完整部署 | **日常开发首选** |
| `orbstack-dev.sh` | OrbStack 开发工具箱：build/deploy/热更新/日志/状态 | 代码改完快速迭代 |
| `run.sh` | 运行 conformance 测试 | 每次验证 |
| `setup.sh` | 通用部署：不构建镜像，适用于 CI 或远程集群 | CI / 生产集群 |
| `cleanup.sh` | 清理测试环境（不删 CRD） | 环境重置 |

---

## 常用工作流

### 首次搭环境（OrbStack）

```bash
# 构建镜像 + 部署所有组件 + 等待就绪
./scripts/setup-orbstack.sh

# 跑一个冒烟测试确认环境正常
./scripts/orbstack-dev.sh test --run-test HTTPRouteSimpleSameNamespace
```

### 代码修改后更新

```bash
# 方式 1：标准（Docker build + rollout）
./scripts/orbstack-dev.sh rebuild        # controller + gateway
./scripts/orbstack-dev.sh rebuild g      # 只更新 gateway
./scripts/orbstack-dev.sh rebuild c      # 只更新 controller

# 方式 2：极速热更新（cargo zigbuild 本地编译 → kubectl cp → pkill，秒级）
# 需要: brew install zig && cargo install cargo-zigbuild && rustup target add aarch64-unknown-linux-gnu
./scripts/orbstack-dev.sh localpatch g   # 只热更新 gateway
./scripts/orbstack-dev.sh localpatch c   # 只热更新 controller
./scripts/orbstack-dev.sh localpatch     # 两个都热更新
```

### 跑 conformance 测试

```bash
# OrbStack 全量测试 — 直接用 run.sh，不需要 port-forward
# OrbStack 原生支持 macOS 直接访问 K8s ClusterIP，直连最稳定
bash scripts/run.sh

# 单个测试
bash scripts/run.sh --run-test HTTPRouteSimpleSameNamespace

# 用 GO_RUN_FILTER 做更灵活的正则匹配
GO_RUN_FILTER="TestConformance/HTTPRoute.*Redirect" bash scripts/run.sh

# 非 OrbStack 集群（Pod IP 不可达时才需要 port-forward）
bash scripts/run.sh --port-forward

# TLSRoute 专用（需先用 --tls-only 初始化环境，见下）
GO_RUN_FILTER="TestConformance/TLSRouteSimpleSameNamespace$" \
  bash scripts/run.sh --conformance-profiles GATEWAY-TLS
```

> **⚠️ 注意**：`orbstack-dev.sh test` 内部强制加了 `--port-forward`。
> 全量测试时应直接使用 `bash scripts/run.sh`，避免 port-forward 在
> 长时间运行（~12 分钟）中断开导致大量 `connection refused` 失败。

### TLSRoute 测试环境

TLSRoute 的 443 端口 passthrough listener 与默认 HTTPS listener 冲突，需要独立环境：

```bash
# 先重新初始化为 TLS 专用模式
./scripts/setup-orbstack.sh --tls-only --skip-build

# 再跑 TLSRoute 测试
./scripts/run.sh --conformance-profiles GATEWAY-TLS
```

### 查看状态和日志

```bash
./scripts/orbstack-dev.sh status          # Pod / GatewayClass / Gateway 状态
./scripts/orbstack-dev.sh logs g          # gateway 实时日志
./scripts/orbstack-dev.sh logs c          # controller 实时日志
./scripts/orbstack-dev.sh shell g         # 进入 gateway pod shell
./scripts/orbstack-dev.sh shell c         # 进入 controller pod shell
```

### 清理环境

```bash
./scripts/cleanup.sh   # 删除 conformance namespace + Edgion 组件，保留 CRD
```

---

## 各脚本详解

### `setup-orbstack.sh` — OrbStack 完整初始化

**做什么**：8 步完整部署流程，专为 OrbStack 本地开发设计：

1. 检查 kubectl context 是否为 `orbstack`
2. 用 `docker build` 构建本地镜像（tag: `pandaala/edgion-*:conformance-dev`，`imagePullPolicy: Never`）
3. 安装 namespace + CRDs（Gateway API experimental v1.5.0 + Edgion CRD）
4. 部署 controller（含 `RUST_BACKTRACE` 注入）
5. 应用 GatewayClass + EdgionGatewayConfig
6. 预创建 conformance 所需的 Gateway CR（避免测试框架自己创建时竞态）
7. 部署 gateway data-plane（restart 以加载预创建 listener）
8. 自动修正 `gateway_address`（读取 edgion-gateway Service 的 ClusterIP，写入 controller ConfigMap，重启 controller，等待 Gateway status.addresses 更新）

**关键参数**：

```bash
./scripts/setup-orbstack.sh                # 全量（debug 构建）
./scripts/setup-orbstack.sh --skip-build   # 跳过构建，直接部署（镜像已存在时）
./scripts/setup-orbstack.sh --rebuild      # 强制 no-cache 重建镜像
./scripts/setup-orbstack.sh --release      # release 构建（更快，无 backtrace）
./scripts/setup-orbstack.sh --tls-only     # TLSRoute 专用环境（不创建 HTTPS listener）
```

**为什么有 gateway_address 修正步骤**：conformance 框架从 `Gateway.status.addresses` 读取目标 IP 来发请求。Edgion controller 需要知道 gateway Service 的 ClusterIP 才能写入 status，但 ClusterIP 在 Service 创建后才确定，因此需要 setup 脚本在 Service 就绪后主动写入 ConfigMap 并重启 controller。

---

### `orbstack-dev.sh` — 开发迭代工具箱

**做什么**：日常开发中最常用的工具，封装了从构建到调试的所有子命令。

**命令速查**：

| 命令 | 说明 |
|------|------|
| `build [c\|g\|all]` | 只 `docker build`，不 rollout |
| `deploy [c\|g\|all]` | 只 rollout（更新镜像 tag + restart），不构建 |
| `rebuild [c\|g\|all]` | `build` + `deploy` 一步完成 |
| `localpatch [c\|g\|all]` | **最快**：本地 zigbuild 编译 → `kubectl cp` → `pkill`，无 Docker |
| `hotpatch [c\|g]` | 从 Docker 镜像提取 binary → `kubectl cp` → `pkill` |
| `test [run.sh 参数]` | 透传给 `run.sh --port-forward`（自动 port-forward 模式） |
| `status` | 打印 Pod / GatewayClass / Gateway 状态摘要 |
| `logs [c\|g]` | `kubectl logs -f` |
| `restart [c\|g\|all]` | `kubectl rollout restart` |
| `shell [c\|g]` | `kubectl exec -it ... /bin/sh` |

**localpatch 前置条件**（一次性）：

```bash
brew install zig
cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu
```

**PROFILE 环境变量**：

```bash
PROFILE=release ./scripts/orbstack-dev.sh rebuild   # release 构建
# 默认 debug（RUST_BACKTRACE=full）
```

---

### `run.sh` — 运行 conformance 测试

**做什么**：调用 `go test` 运行 Gateway API conformance 套件，支持 profile 过滤、单测试运行、自动 port-forward。

**关键参数**：

```bash
./scripts/run.sh                                         # 默认：GATEWAY-HTTP + GATEWAY-GRPC
./scripts/run.sh --conformance-profiles "GATEWAY-HTTP"   # 只跑 HTTP profile
./scripts/run.sh --run-test HTTPRouteHeaderMatching       # 单个测试
./scripts/run.sh --core                                   # 只跑 Core features
./scripts/run.sh --extended                               # Core + Extended features
./scripts/run.sh --port-forward                           # 自动 port-forward（非直连集群）
./scripts/run.sh --no-debug                               # 关闭 debug 输出（减少日志）
./scripts/run.sh --timeout 10m                            # 自定义超时
```

**Transport Access Layer（TAL）相关环境变量**：

```bash
# port-forward 模式手动控制
export GATEWAY_PROXY_HOST=127.0.0.1
export GATEWAY_PROXY_PORT_MAP="80:18080,443:18443,8443:28443"   # 推荐
# 或者分开设置
export GATEWAY_PROXY_HTTP_PORT=18080
export GATEWAY_PROXY_HTTPS_PORT=18443
export GATEWAY_PROXY_HTTPS_8443_PORT=28443

export GATEWAY_PROXY_ENABLE_GRPC=true   # 同时启用 gRPC proxy 客户端
export TAL_DEBUG=true                    # 打印每次地址改写日志
```

**输出**：测试结束后在 `conformance-report.yaml`（项目根目录）生成 YAML 格式的 conformance 报告。

---

### `setup.sh` — 通用部署（CI / 远程集群）

**做什么**：与 `setup-orbstack.sh` 逻辑相同的 8 步部署，但**不构建镜像**，适用于：

- CI 流水线（镜像由 CD 推送到 registry）
- 远程集群（镜像从 registry 拉取）

**与 `setup-orbstack.sh` 的主要区别**：

| | `setup-orbstack.sh` | `setup.sh` |
|---|---|---|
| 构建镜像 | 是（docker build） | 否 |
| imagePullPolicy | `Never` | `Always`（默认） |
| 适用环境 | OrbStack 本地 | CI / 远程集群 |
| context 检查 | 有（检查 orbstack） | 无 |

```bash
./scripts/setup.sh
./scripts/setup.sh --controller-image registry.example.com/edgion-controller:v1.0.0
./scripts/setup.sh --gateway-image registry.example.com/edgion-gateway:v1.0.0
./scripts/setup.sh --pull-policy IfNotPresent
./scripts/setup.sh --tls-only    # TLSRoute 专用模式
./scripts/setup.sh --skip-crd    # 跳过 CRD 安装（已安装时）
```

---

### `cleanup.sh` — 清理环境

**做什么**：删除 conformance 测试创建的所有资源，保留 CRD 和 `edgion-system` namespace。

具体删除范围：
- `gateway-conformance-infra` / `gateway-conformance-app-backend` / `gateway-conformance-web-backend` namespace
- `GatewayClass edgion`
- `EdgionGatewayConfiguration conformance-gateway`
- `edgion-system` 下的 controller / gateway Deployment、Service、ConfigMap

**不删除**（需要手动）：
- `edgion-system` namespace 本身
- Gateway API CRD
- Edgion CRD

```bash
./scripts/cleanup.sh

# 如果需要完全清除（含 CRD）：
kubectl delete namespace edgion-system
kubectl delete -f config/crd/gateway-api/
kubectl delete -f config/crd/edgion-crd/
```

---

## 注意事项

### 预创建 Gateway 可能丢失

`setup-orbstack.sh` 会预创建 6 个 Gateway CR（`same-namespace`、`same-namespace-with-https-listener`、`all-namespaces`、`backend-namespaces`、`client-validation-default`、`client-validation-insecure-fallback`）。

如果做过 `cleanup.sh` 或手动删除操作，部分 Gateway 可能丢失。恢复方法：

```bash
kubectl apply --server-side --force-conflicts -f manifests/conformance-gateway.yaml
# 重启 gateway 以绑定新端口（如 8443）
./scripts/orbstack-dev.sh restart g
```

### go test 输出完全缓冲

全量测试运行约 12 分钟，期间终端**没有任何输出**（go test 对子包的 `-v` 输出做完整缓冲，直到测试结束后一次性打印）。这是正常的，不要误以为卡住了。

### conformance report 单测模式的陷阱

用 `--run-test` 或 `GO_RUN_FILTER` 跑单个测试时，生成的 `conformance-report.yaml` 会把所有**未运行**的测试标记为 Passed。这不代表全量通过，只是 conformance 框架的行为。只有全量运行的 report 才是真实的。

---

## 常见问题

**Q: `setup-orbstack.sh` 里为什么要手动修正 gateway_address？**

Edgion controller 把 `gateway_address`（即 edgion-gateway Service 的 ClusterIP）写入 Gateway 的 `status.addresses`，conformance 框架用它来发测试请求。但 ClusterIP 在 Service 创建之前不存在，所以 setup 脚本必须在 Service 创建后读取并注入。

**Q: 为什么需要预创建 conformance Gateway CR？**

Edgion gateway 进程在启动时绑定 listener 端口，之后不支持动态新增端口。conformance 框架会在测试中创建 Gateway CR，如果此时 gateway 进程已启动，新 listener 无法绑定。因此 setup 脚本预先创建 Gateway CR，让 gateway 启动时就读取到完整的 listener 配置。

**Q: TLSRoute 为什么需要独立环境（`--tls-only`）？**

默认环境在 443 端口部署了 HTTPS listener（用于 `HTTPRouteHTTPS*` 测试）。TLSRoute 需要在同一 443 端口做 TLS passthrough，两者冲突。`--tls-only` 会跳过 HTTPS listener，只创建 TLS passthrough Gateway。

**Q: OrbStack 下应该用 `orbstack-dev.sh test` 还是直接 `run.sh`？**

- **单个测试**：`orbstack-dev.sh test --run-test <Name>` 方便，port-forward 时间短不会断
- **全量测试**：直接 `bash scripts/run.sh`。`orbstack-dev.sh test` 强制加 `--port-forward`，长时间运行容易断开
