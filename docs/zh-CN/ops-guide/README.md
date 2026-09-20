# 运维指南 (Ops Guide)

本章节面向运维人员（Operator），介绍 Gateway 的部署、配置和管理。

## 目录

### Gateway 配置
- [Gateway 资源总览](./gateway/overview.md)：Gateway 与 listener 配置入口。
- [GatewayClass 配置](./gateway/gateway-class.md)：控制器绑定与参数管理。
- [HTTP to HTTPS 重定向 🔌](./gateway/http-to-https-redirect.md)：Gateway 级重定向策略。
- [Preflight 策略 🔌](./gateway/preflight-policy.md)：Gateway 级 OPTIONS 预检处理。

### 监听器

- [HTTP 监听器](./gateway/listeners/http.md)：明文 HTTP 接入。
- [HTTPS 监听器](./gateway/listeners/https.md)：TLS 终结与证书配置。
- [TCP 监听器](./gateway/listeners/tcp.md)：四层 TCP 接入。
- [gRPC 监听器](./gateway/listeners/grpc.md)：基于 HTTP/HTTPS listener 承载 gRPC。

### TLS

- [TLS 终结](./gateway/tls/tls-termination.md)：标准 TLS 终结配置。
- [EdgionTls 扩展 🔌](./gateway/tls/edgion-tls.md)：扩展证书与 TLS 能力。
- [ACME 自动证书 🔌](./gateway/tls/acme.md)：自动签发与续期。
- [FIPS 模式 🔒](./gateway/tls/fips.md)：BoringSSL FIPS 模式构建与默认批准的密码套件 / 曲线集合。

### 基础设施

- [Secret 管理](./infrastructure/secret-management.md)：证书与凭据管理。
- [跨命名空间引用 (ReferenceGrant)](./infrastructure/reference-grant.md)：跨命名空间资源授权。
- [mTLS 配置](./infrastructure/mtls.md)：网关到后端的双向 TLS。
- [连接器 (LinkSys) 🔌](./infrastructure/connectors.md)：外部系统集成（Redis、Etcd、Elasticsearch、Webhook、Kafka）。
- [服务发现集成](./infrastructure/service-discovery.md)：集成外部服务注册中心（Consul、Nacos、Eureka、ZooKeeper）。

### 可观测性

- [访问日志](./observability/access-log.md)：访问日志字段与输出（含 AI `genai` 账本）。
- [监控指标](./observability/metrics.md)：Prometheus 指标采集与告警建议。
- [指标清单](./observability/metrics-inventory.md)：全部 `edgion_*` 指标目录（与英文权威清单对称；CI 校验英文版）。

### 配置参考

- [Controller 配置](./configuration/controller-config.md)：Controller YAML 进程配置参数。
- [Gateway 配置](./configuration/gateway-config.md)：Gateway YAML 进程配置参数。
- [EdgionGatewayConfig](./configuration/edgion-gateway-config.md)：GatewayClass 级运行时配置（超时、真实 IP、全局插件等）。

### 插件策略

- [插件策略（允许清单 / 拒绝清单）](./plugin-policy.md)：在 EdgionGatewayConfig 中配置 GatewayClass 作用域执行策略，支持动态 Block/Bypass 更新及无效配置保留边界。
- [未知配置字段检测](./plugin-config-unknown-fields.md)：检测 `EdgionPlugins`、`EdgionStreamPlugins`、类型化 `EdgionConfigData` 和 `LinkSys` 配置块中无法识别的键——通过日志、状态条件和诊断接口发出警告，从不阻断；并提供 `POST /api/v1/validate/{kind}` 接口用于搭建你自己的阻断关卡。

### 联邦

- [联邦 RBAC](./federation-rbac.md)：授权 Center 对 Controller 进行操作——内置默认策略、示例与安全说明。同时涵盖 [CLI Token RBAC](./federation-rbac.md#cli-token-rbac)（用于 CI/自动化的静态 Bearer Token）。

### 运维工具

- [Linux 内核调优](./kernel-tuning.md)：可选 FD/backlog 基线、按需调优、验证和回滚。
- [edgion-cli](./edgion-cli.md)：资源管理与调试 CLI。
