# Edgion K8s Integration Testing Guide

> 参考 `/Users/caohao/ws1/Edgion/skills/integration-testing.md`，本文件只关注 **Kubernetes 场景的差异与改造要求**。

## 1. 目标与范围

本指南用于把原有本地进程式集成测试迁移到 K8s 环境，重点是：

- 配置层：`examples/test/conf` -> `examples/k8stest/conf`
- 执行层：本地进程 -> K8s Deployment/Service
- 断言层：固定 IP/端口假设 -> Service/EndpointSlice 动态行为

## 2. 与原集成测试的核心差异

| 维度 | 原模式（local process） | K8s 模式 |
|---|---|---|
| 进程模型 | `start_all_with_conf.sh` 拉起 controller/gateway/test_server | 由 Deployment 管理 Pod 生命周期 |
| 后端寻址 | `127.0.0.x` / `localhost` / 固定端口 | `*.svc.cluster.local` + Service 端口 |
| Endpoint 资源 | 常用手工 `Endpoint/EndpointSlice` fixture | 不应手工维护，交由 K8s 根据 Service selector 自动生成 |
| 可用实例数 | 常固定、可预期 | Pod 重建/扩缩容后 IP 会变 |
| 配置载入 | 直接 apply 原 conf | 需要 K8s 化 conf（后端/service 语义转换） |
| 测试断言 | 可断言固定 IP | 应断言“属于当前 endpoint 集合/满足分布” |

## 3. K8s 网络与测试约束

### 3.1 Endpoint/EPS 不再是配置输入

- 测试配置中不应再维护 `kind: Endpoint` / `kind: EndpointSlice`。
- 只保留 Service + Deployment（或 StatefulSet）。
- 需要多副本行为（RR/CH）时，增加 test-server 副本数，而不是写死 endpoint IP。

### 3.2 Upstream 地址与端口

原来的典型 upstream：

- `127.0.0.1:30001`~`30005`
- `localhost:30040`

K8s 模式建议统一改为：

- `edgion-test-server.edgion-test.svc.cluster.local:<port>`
- 或同 namespace 下短名 service（需确保 DNS 可解析范围一致）

### 3.3 断言策略变化

- 不再断言“必须是某个固定 IP”。
- 改为断言：
  - IP 在当前 endpoints 集合内；
  - 或按指标验证分布（RR/CH）；
  - 或按 header/body 校验功能逻辑，不绑定实例地址。

## 4. 当前仓库现状（已扫描）

### 4.1 `examples/k8stest/conf` 覆盖缺口

当前 `examples/k8stest/conf` 缺失以下关键目录（相对 `examples/test/conf`）：

- `EdgionPlugins/*`（整组缺失，含 `DebugAccessLog/JwtAuth/JweDecrypt/HmacAuth/HeaderCertAuth/...`）
- `Gateway/PortConflict`
- `Gateway/StreamPlugins`
- `HTTPRoute/Backend/LBRoundRobin`
- `HTTPRoute/Backend/LBConsistentHash`
- `TCPRoute/StreamPlugins`
- `ref-grant-status`

并存在命名不一致：

- `HTTPRoute/Backend/LBPolicy`（与原测试 `LBRoundRobin`/`LBConsistentHash` 不一致）
- `Gateway/Plugins`（与 `EdgionPlugins/*`、`Gateway/StreamPlugins` 语义需明确拆分）

### 4.2 执行链与配置源不一致

`/Users/caohao/ws1/Edgion/examples/k8stest/scripts/run_k8s_integration.sh` 只是 wrapper；
当前实际执行脚本（deploy 仓库）仍优先读取 `examples/test/conf`，尚未切到 `examples/k8stest/conf` 作为唯一数据源。

## 5. 新的 conf 需要改哪些（落地清单）

### 5.1 目录与命名对齐（P0）

- 在 `examples/k8stest/conf` 补齐与 `examples/test/conf` 同名 suite 目录。
- `LBPolicy` 拆分或映射为：
  - `HTTPRoute/Backend/LBRoundRobin`
  - `HTTPRoute/Backend/LBConsistentHash`
- 补齐 `EdgionPlugins/base` 与所有插件子目录。

### 5.2 资源内容转换（P0）

- 删除/禁止 Endpoint 与 EndpointSlice 文档。
- 所有 backendRef 指向 Service（名称+端口）。
- 为需要多实例测试的后端准备独立 Deployment（建议 3 副本）。
- 保持 Gateway/Route 的非 backend 语义尽量不变（host/match/filter 保持一致）。

### 5.3 命名空间与基础资源（P0）

建议固定三套 namespace：

- `edgion-system`: controller/gateway/RBAC/CRD 相关
- `edgion-default`: 路由与插件配置资源（默认测试配置）
- `edgion-test`: test-server/test-client/service

并确保 base conf 最小集齐全：

- `GatewayClass`
- `EdgionGatewayConfig`
- 测试所需 TLS Secret / ReferenceGrant

### 5.4 K8s 依赖场景（P1）

- LDAP、OIDC、ForwardAuth、WebhookKeyGet 等依赖外部服务的套件，需要在 K8s 下提供对应 Service。
- 如仍依赖宿主机容器（docker compose），需标注为“非纯 K8s 套件”。

## 6. suite 需要加什么 / 改什么

### 6.1 必做改造（P0）

以下类型的 suite 需改为“环境无关”写法：

- 禁止硬编码 `http://127.0.0.1:<port>`。
- 统一使用 `TestContext`（`ctx.target_host` + `ctx.*_port`）构造 URL。
- 指标抓取使用 `ctx.metrics_client()`，不要写死 `127.0.0.1:5901`。

### 6.2 重点套件改造（P0/P1）

- `DirectEndpoint`: 当前只支持单 IP 注入（`EDGION_TEST_DIRECT_ENDPOINT_IP`），应扩展为“从 Admin API 拉取 endpoint 列表并选择有效 IP”。
- `AllEndpointStatus`: 断言改为集合/统计断言，避免依赖固定实例。
- `HTTPRoute/Backend/LBRoundRobin`、`LBConsistentHash`: 分布断言基于实时 endpoint 集合。
- `Gateway/StreamPlugins`、`TCPRoute/StreamPlugins`: 避免本机 loopback 特定假设。
- `DynamicExternalUpstream`: 将 `localhost` 回环拒绝场景拆分为 local-only case，不作为 K8s 默认通过条件。

### 6.3 推荐新增 suite（K8s 专属）

- `K8S/ServiceRollingUpdate`: 验证 Pod 轮换期间路由与重试行为。
- `K8S/EndpointChurn`: 验证 endpoint 增减时 DirectEndpoint/负载均衡行为。
- `K8S/NamespaceIsolation`: 验证跨 namespace backendRef + ReferenceGrant 行为。

## 7. 执行建议（先通后全）

按以下阶段推进：

1. P0：`HTTPRoute/Basic/Match/WeightedBackend/Timeout`, `GRPCRoute/Basic/Match`, `UDPRoute/Basic`, `EdgionTls/*`。
2. P1：`EdgionPlugins` 核心插件（Jwt/Jwe/Hmac/HeaderCert/KeyAuth/BasicAuth/RateLimit/RealIp）。
3. P2：`DirectEndpoint/AllEndpointStatus/StreamPlugins/Ldap/OpenidConnect`。

### 推荐执行模型（已在脚本实现）

- 阶段 1：`prepare`
  - 校验 `k8stest/conf` 不含 Endpoint/EndpointSlice
  - 严格 apply 全量配置（失败即退出）
  - apply 完成后统一重启 gateway 并等待 ready
- 阶段 2：`run tests`
  - 只执行 test_client 套件，不再每个 suite 重启 gateway

对应脚本：

- `/Users/caohao/ws1/Edgion/examples/k8stest/scripts/run_k8s_integration.sh`
  - `--prepare-only`：只准备环境
  - `--skip-prepare`：只跑测试

## 8. 受影响 suite 快速索引（代码扫描）

以下目录中仍存在 `127.0.0.1/localhost/固定 admin 端口` 假设，建议优先改造：

- `/Users/caohao/ws1/Edgion/examples/code/client/suites/http_route/backend/*`
- `/Users/caohao/ws1/Edgion/examples/code/client/suites/edgion_plugins/direct_endpoint/*`
- `/Users/caohao/ws1/Edgion/examples/code/client/suites/edgion_plugins/all_endpoint_status/*`
- `/Users/caohao/ws1/Edgion/examples/code/client/suites/edgion_plugins/dynamic_external_upstream/*`
- `/Users/caohao/ws1/Edgion/examples/code/client/suites/gateway/allowed_routes/*`
- `/Users/caohao/ws1/Edgion/examples/code/client/suites/gateway/listener_hostname/*`
- `/Users/caohao/ws1/Edgion/examples/code/client/suites/gateway/combined/*`
- `/Users/caohao/ws1/Edgion/examples/code/client/suites/gateway/dynamic/*`
- `/Users/caohao/ws1/Edgion/examples/code/client/suites/tcp_route/stream_plugins/*`

## 9. 与原文档的对应关系

- 原流程与测试框架说明：`/Users/caohao/ws1/Edgion/skills/integration-testing.md`
- 本文仅补充 K8s 差异、K8s conf 清单与 suite 改造项。
