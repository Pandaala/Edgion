---
name: 03-resources
description: 所有 Kubernetes 资源的功能与配置 Schema 参考。编号与 01-architecture/05-resources 对齐，按资源类型统一组织。
---

# 03 资源功能参考

> 所有 Gateway API 标准资源和 Edgion 扩展资源的**外部契约**：完整 YAML Schema、字段类型与默认值、配置示例。
> 资源的**内部实现**（Handler 流程、requeue 关联、源码位置）见 [01-architecture/05-resources/](../../01-architecture/05-resources/SKILL.md)——两边编号一一对齐，改代码通常两边都要看。

## 编号对齐说明

| # | 资源 | 架构文档 | 功能文档（本目录） |
|---|------|---------|-------------------|
| 00 | 通用概念 | `05-resources/00-resource-flow.md` | [00-common-concepts.md](00-common-concepts.md) |
| 01 | Gateway | `05-resources/01-gateway.md` | [01-gateway.md](01-gateway.md) |
| 02 | GatewayClass | `05-resources/02-gateway-class.md` | [02-gateway-class.md](02-gateway-class.md) |
| 03 | EdgionGatewayConfig | `05-resources/03-edgion-gateway-config.md` | → [../02-config/02-edgion-gateway-config.md](../02-config/02-edgion-gateway-config.md) |
| 04 | HTTPRoute | `05-resources/04-http-route.md` | [04-httproute.md](04-httproute.md) |
| 05 | GRPCRoute | `05-resources/05-grpc-route.md` | [05-grpcroute.md](05-grpcroute.md) |
| 06 | TCPRoute | `05-resources/06-tcp-route.md` | [06-tcproute.md](06-tcproute.md) |
| 07 | TLSRoute | `05-resources/07-tls-route.md` | [07-tlsroute.md](07-tlsroute.md) |
| 08 | UDPRoute | `05-resources/08-udp-route.md` | [08-udproute.md](08-udproute.md) |
| 09 | EdgionTls | `05-resources/09-edgion-tls.md` | [09-edgion-tls.md](09-edgion-tls.md) |
| 12 | BackendTLSPolicy | `05-resources/12-backend-tls-policy.md` | [12-backend-tls-policy.md](12-backend-tls-policy.md) |
| 13 | EdgionPlugins | `05-resources/13-edgion-plugins.md` | [13-edgion-plugins.md](13-edgion-plugins.md) |
| 14 | EdgionStreamPlugins | `05-resources/14-edgion-stream-plugins.md` | [14-edgion-stream-plugins.md](14-edgion-stream-plugins.md) |
| 16 | Service/Endpoints | `05-resources/16-service-endpoints.md` | [16-service-backends.md](16-service-backends.md) |
| 17 | EdgionAcme | `05-resources/17-edgion-acme.md` | [17-acme.md](17-acme.md) |
| 18 | LinkSys | `05-resources/18-link-sys.md` | [18-link-sys.md](18-link-sys.md) |

> 编号 03（EdgionGatewayConfig）的功能文档在 `02-config/` 下，因为它本质是配置层资源。
> 编号 10（Secret）、11（ReferenceGrant）、15（PluginMetaData）暂无独立功能文档，参见架构文档。

## 按分类浏览

### 网关入口

| 资源 | 说明 |
|------|------|
| [01-gateway.md](01-gateway.md) | Listener 配置、协议、端口、TLS 绑定、AllowedRoutes |
| [02-gateway-class.md](02-gateway-class.md) | GatewayClass 与 parametersRef |

### 路由

| 资源 | 说明 |
|------|------|
| [00-common-concepts.md](00-common-concepts.md) | parentRef/backendRef 通用概念、跨命名空间、hostname 交集 |
| [04-httproute.md](04-httproute.md) | HTTP 路由匹配、Filter、超时重试、Session Persistence |
| [05-grpcroute.md](05-grpcroute.md) | gRPC 路由匹配、gRPC-Web |
| [06-tcproute.md](06-tcproute.md) | TCP 路由、Stream 插件注解 |
| [07-tlsroute.md](07-tlsroute.md) | TLS Passthrough 路由、SNI 匹配 |
| [08-udproute.md](08-udproute.md) | UDP 路由 |

### TLS 与证书

| 资源 | 说明 |
|------|------|
| [09-edgion-tls.md](09-edgion-tls.md) | 扩展 TLS：mTLS、版本、密码套件、OCSP |
| [12-backend-tls-policy.md](12-backend-tls-policy.md) | 上游 TLS 校验、CA、SAN |
| [17-acme.md](17-acme.md) | ACME 自动证书：HTTP-01/DNS-01、续期、存储 |

### 插件

| 资源 | 说明 |
|------|------|
| [13-edgion-plugins.md](13-edgion-plugins.md) | 28 个 HTTP 插件目录与配置 |
| [14-edgion-stream-plugins.md](14-edgion-stream-plugins.md) | Stream 插件两阶段模型 |

### 后端与外部系统

| 资源 | 说明 |
|------|------|
| [16-service-backends.md](16-service-backends.md) | Service 发现、健康检查、负载均衡策略 |
| [18-link-sys.md](18-link-sys.md) | Redis/Etcd/ES/Webhook/Kafka 连接器 |

## 快速定位

| 你想… | 直接打开 |
|-------|---------|
| 配置 Gateway Listener | [01-gateway.md](01-gateway.md) |
| 配置 HTTP 路由规则 | [04-httproute.md](04-httproute.md) |
| 配置 gRPC 路由 | [05-grpcroute.md](05-grpcroute.md) |
| 配置 TLS/mTLS | [09-edgion-tls.md](09-edgion-tls.md) |
| 配置自动证书 | [17-acme.md](17-acme.md) |
| 查找 HTTP 插件 | [13-edgion-plugins.md](13-edgion-plugins.md) |
| 配置 TCP 层插件 | [14-edgion-stream-plugins.md](14-edgion-stream-plugins.md) |
| 配置健康检查/负载均衡 | [16-service-backends.md](16-service-backends.md) |
| 接入外部系统 | [18-link-sys.md](18-link-sys.md) |
| 配置上游 TLS | [12-backend-tls-policy.md](12-backend-tls-policy.md) |
