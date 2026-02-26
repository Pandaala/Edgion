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

### 基础设施

- [Secret 管理](./infrastructure/secret-management.md)：证书与凭据管理。
- [跨命名空间引用 (ReferenceGrant)](./infrastructure/reference-grant.md)：跨命名空间资源授权。
- [mTLS 配置](./infrastructure/mtls.md)：网关到后端的双向 TLS。

### 可观测性

- [访问日志](./observability/access-log.md)：访问日志字段与输出。
- [监控指标](./observability/metrics.md)：Prometheus 指标采集与告警建议。

### 运维工具

- [edgion-ctl](./edgion-ctl.md)：资源管理与调试 CLI。
