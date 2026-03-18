# Gateway API 支持

> Edgion 对 Kubernetes Gateway API v1.4.0 标准的支持范围、资源映射、一致性测试和 Edgion 扩展点。

## 支持的 Gateway API 资源

| Gateway API Resource | Edgion 支持 | Channel | 说明 |
|---------------------|------------|---------|------|
| `GatewayClass` | ✅ | Standard | 网关类声明 |
| `Gateway` | ✅ | Standard | 网关实例，包含 Listener 配置 |
| `HTTPRoute` | ✅ | Standard | HTTP 路由规则 |
| `GRPCRoute` | ✅ | Standard | gRPC 路由规则 |
| `TCPRoute` | ✅ | Experimental | TCP 四层路由 |
| `TLSRoute` | ✅ | Experimental | TLS passthrough/terminate 路由 |
| `UDPRoute` | ✅ | Experimental | UDP 四层路由 |
| `ReferenceGrant` | ✅ | Standard | 跨命名空间引用授权 |
| `BackendTLSPolicy` | ✅ | Standard | 上游 mTLS 策略 |

## CRD 安装

```bash
# Gateway API 标准 CRD
kubectl apply -f config/crd/gateway-api/gateway-api-standard-v1.4.0.yaml

# Edgion 扩展 CRD
kubectl apply -f config/crd/edgion-crd/
```

## Edgion 扩展 CRD

| CRD | 说明 |
|-----|------|
| `EdgionGatewayConfig` | 全局网关配置（对应 Controller 侧的 base config） |
| `EdgionPlugins` | HTTP 层插件配置（认证、限流、改写等） |
| `EdgionStreamPlugins` | TCP 层插件配置（IP 过滤等） |
| `EdgionTls` | TLS 证书和配置 |
| `PluginMetaData` | 插件元数据（共享配置） |
| `LinkSys` | 外部系统连接（Redis/Etcd/ES/Kafka） |
| `EdgionAcme` | ACME 自动证书 |

## Gateway API 一致性测试

```bash
cd examples/gateway-api-conformance
go test -v ./... -run TestGatewayAPIConformance
```

基于 `sigs.k8s.io/gateway-api v1.4.0` 的官方一致性测试框架，Go 实现。

**目录**: `examples/gateway-api-conformance/`

## Edgion 扩展点

Edgion 通过以下方式扩展标准 Gateway API：

### 1. Annotations（`edgion.io/*`）
在 Gateway/Listener/Route 级别通过 annotation 注入 Edgion 特有配置。
参考：[development/05-annotations-reference.md](../development/05-annotations-reference.md)

### 2. ExtensionRef Filters
HTTPRoute/GRPCRoute 的 `filters` 中使用 `ExtensionRef` 引用 Edgion 资源：
- `EdgionPlugins` — 插件链
- `LoadBalancer` — LB 策略

### 3. 自定义 CRD
`EdgionPlugins`、`EdgionStreamPlugins`、`EdgionTls`、`LinkSys` 等 CRD 提供标准 Gateway API 未涵盖的能力。

> **🔌 Edgion Extension**
>
> 文档中凡涉及 Edgion 扩展功能（自定义 Annotation、扩展 CRD 等），均使用此标记区分。

## Key Files

- `config/crd/gateway-api/` — Gateway API 标准 CRD YAML
- `config/crd/edgion-crd/` — Edgion 扩展 CRD YAML
- `src/types/resources/` — 所有资源的 Rust 类型定义
- `examples/gateway-api-conformance/` — 一致性测试（Go）
