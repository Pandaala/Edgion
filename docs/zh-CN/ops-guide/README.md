# 运维指南 (Ops Guide)

本章节面向运维人员（Operator），介绍 Gateway 的部署、配置和管理。

## 目录

### Gateway 配置
- [Gateway 资源总览](./gateway/overview.md)
- [GatewayClass 配置](./gateway/gateway-class.md)
- [HTTP to HTTPS 重定向 🔌](./gateway/http-to-https-redirect.md)
- [Preflight 策略 🔌](./gateway/preflight-policy.md)
- **监听器配置**
  - [HTTP 监听器](./gateway/listeners/http.md)
  - [HTTPS 监听器](./gateway/listeners/https.md)
  - [TCP 监听器](./gateway/listeners/tcp.md)
  - [gRPC 监听器](./gateway/listeners/grpc.md)
- **TLS 配置**
  - [TLS 终结](./gateway/tls/tls-termination.md)
  - [EdgionTls 扩展 🔌](./gateway/tls/edgion-tls.md)

### 基础设施
- [Secret 管理](./infrastructure/secret-management.md)
- [跨命名空间引用 (ReferenceGrant)](./infrastructure/reference-grant.md)
- [mTLS 配置](./infrastructure/mtls.md)

### 可观测性
- [访问日志](./observability/access-log.md)
- [监控指标](./observability/metrics.md)
