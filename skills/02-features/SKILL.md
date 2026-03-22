---
name: features
description: 功能深度参考 skill。面向用户/运维视角，涵盖二进制启动与部署、配置 Schema、资源功能 Schema、可观测性、注解的完整功能文档。
---

# 02 功能与配置参考

> Edgion 各功能模块的深度文档，面向**用户和运维**视角。
> 架构实现细节见 `../01-architecture/SKILL.md`，编码规范见 `../03-coding/SKILL.md`。

## 设计原则

- **Schema 驱动**：每个资源、每个配置都有完整的字段定义和类型说明
- **Gateway API v1.4 对齐**：Gateway API 标准资源引用 v1.4.0 规范，标注 Edgion 扩展点
- **与架构编号对齐**：`03-resources/` 编号与 `01-architecture/05-resources/` 一一对应
- **用户视角**：关注"怎么用"和"怎么配"，而非"内部怎么实现"

## 目录总览

| # | 目录 | 用途 |
|---|------|------|
| 01 | [binary-and-deployment/](01-binary-and-deployment/SKILL.md) | 三个 bin 的启动方式、CLI 参数、部署模式、Feature Flags |
| 02 | [config/](02-config/SKILL.md) | Controller/Gateway TOML 配置 Schema、EdgionGatewayConfig CRD Schema |
| 03 | [resources/](03-resources/SKILL.md) | **所有资源功能 Schema**：Gateway、Route、TLS、Plugin、Backend、LinkSys |
| 04 | [observability/](04-observability/SKILL.md) | 可观测性功能：Access Log 配置、Metrics 端点、协议日志 |
| 05 | [annotations/](05-annotations/SKILL.md) | 注解与 Options 参考：所有 `edgion.io/*` 键的完整列表 |

## 快速定位

| 你想… | 从这里开始 |
|-------|-----------|
| 了解如何启动 Controller/Gateway | [01-binary-and-deployment/](01-binary-and-deployment/SKILL.md) |
| 修改 TOML 配置文件 | [02-config/](02-config/SKILL.md) |
| 配置 Gateway Listener | [03-resources/01-gateway.md](03-resources/01-gateway.md) |
| 配置 HTTP 路由规则 | [03-resources/04-httproute.md](03-resources/04-httproute.md) |
| 配置 gRPC 路由 | [03-resources/05-grpcroute.md](03-resources/05-grpcroute.md) |
| 配置 TLS 证书/mTLS | [03-resources/09-edgion-tls.md](03-resources/09-edgion-tls.md) |
| 使用自动证书（ACME） | [03-resources/17-acme.md](03-resources/17-acme.md) |
| 查找某个插件的配置 | [03-resources/13-edgion-plugins.md](03-resources/13-edgion-plugins.md) |
| 配置健康检查/负载均衡 | [03-resources/16-service-backends.md](03-resources/16-service-backends.md) |
| 配置 Access Log 输出 | [04-observability/00-logging.md](04-observability/00-logging.md) |
| 接入 Redis/ES 等外部系统 | [03-resources/18-link-sys.md](03-resources/18-link-sys.md) |
| 查找某个注解的含义 | [05-annotations/](05-annotations/SKILL.md) |

## Gateway API 版本

Edgion 基于 **Gateway API v1.4.0**，支持范围：

| 资源 | API Version | 支持状态 |
|------|-------------|---------|
| Gateway | `gateway.networking.k8s.io/v1` | Core |
| GatewayClass | `gateway.networking.k8s.io/v1` | Core |
| HTTPRoute | `gateway.networking.k8s.io/v1` | Core |
| GRPCRoute | `gateway.networking.k8s.io/v1` | Core |
| ReferenceGrant | `gateway.networking.k8s.io/v1beta1` | Core |
| TCPRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |
| TLSRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |
| UDPRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |
| BackendTLSPolicy | `gateway.networking.k8s.io/v1alpha3` | Experimental |

Edgion 扩展资源（API group: `edgion.io`）：

| 资源 | API Version | 用途 |
|------|-------------|------|
| EdgionGatewayConfig | `edgion.io/v1alpha1` | GatewayClass 级别运行时配置 |
| EdgionTls | `edgion.io/v1` | 扩展 TLS 配置（mTLS、版本、密码套件） |
| EdgionPlugins | `edgion.io/v1` | HTTP 插件配置 |
| EdgionStreamPlugins | `edgion.io/v1` | TCP/TLS 层插件配置 |
| EdgionAcme | `edgion.io/v1` | ACME 自动证书管理 |
| LinkSys | `edgion.io/v1` | 外部系统连接器 |
| PluginMetaData | `edgion.io/v1` | 插件元数据 |
