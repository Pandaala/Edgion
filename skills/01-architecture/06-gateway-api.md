---
name: gateway-api-conformance
description: Gateway API v1.4.0 合规性：支持的资源、Edgion 扩展 CRD、一致性测试、扩展点。
---

# Gateway API 合规性

Edgion 实现了 Kubernetes Gateway API v1.4.0 规范，支持 HTTP、gRPC、TCP、TLS 和 UDP 路由协议。在标准 Gateway API 基础上，Edgion 通过自定义 CRD 和注解提供扩展功能。

## 支持的 Gateway API 资源

| 资源 | API Group | Channel | 说明 |
|------|-----------|---------|------|
| **GatewayClass** | gateway.networking.k8s.io/v1 | Standard | 定义 Gateway 实现类，Edgion 的 controllerName 为 `edgion.io/gateway-controller` |
| **Gateway** | gateway.networking.k8s.io/v1 | Standard | 定义监听器、端口、TLS 配置，一个 Gateway 对应一组逻辑入口 |
| **HTTPRoute** | gateway.networking.k8s.io/v1 | Standard | HTTP 路由规则：主机名匹配、路径匹配、Header 匹配、请求重定向/重写/镜像 |
| **GRPCRoute** | gateway.networking.k8s.io/v1 | Standard | gRPC 路由规则：Service/Method 匹配、Header 匹配 |
| **TCPRoute** | gateway.networking.k8s.io/v1alpha2 | Experimental | TCP 四层路由，按端口转发到后端 |
| **TLSRoute** | gateway.networking.k8s.io/v1alpha2 | Experimental | TLS passthrough 路由，按 SNI 匹配后直接转发加密流量 |
| **UDPRoute** | gateway.networking.k8s.io/v1alpha2 | Experimental | UDP 路由，按端口转发 |
| **ReferenceGrant** | gateway.networking.k8s.io/v1beta1 | Standard | 跨命名空间引用授权，仅在 Controller 侧使用（no_sync_kinds） |
| **BackendTLSPolicy** | gateway.networking.k8s.io/v1alpha3 | Experimental | 后端 TLS 策略，配置 Gateway→Backend 的 mTLS |

## CRD 安装

Gateway API 标准 CRD 通过官方 YAML 安装：

```bash
# 安装 Standard channel CRDs
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.4.0/standard-install.yaml

# 安装 Experimental channel CRDs（包含 TCPRoute、TLSRoute、UDPRoute 等）
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.4.0/experimental-install.yaml
```

Edgion 扩展 CRD 通过 Edgion 自有的部署清单安装。

## Edgion 扩展 CRD

| 资源 | API Group | 说明 |
|------|-----------|------|
| **EdgionGatewayConfig** | edgion.io/v1alpha1 | 全局/per-Gateway 配置：超时、缓冲区大小、日志级别、全局插件 |
| **EdgionPlugins** | edgion.io/v1alpha1 | HTTP 插件集合：可通过 HTTPRoute/GRPCRoute 的 ExtensionRef 引用 |
| **EdgionStreamPlugins** | edgion.io/v1alpha1 | TCP/UDP 流插件集合：连接级别过滤（IP 黑白名单等） |
| **EdgionTls** | edgion.io/v1alpha1 | TLS 证书和密钥资源：替代标准 Secret，支持文件路径引用和内联证书 |
| **PluginMetaData** | edgion.io/v1alpha1 | 插件元数据：描述插件能力、参数 schema、版本信息 |
| **LinkSys** | edgion.io/v1alpha1 | 外部系统集成：ES、Redis、Etcd、Webhook、File 等数据源连接配置 |
| **EdgionAcme** | edgion.io/v1alpha1 | ACME 自动证书签发配置：Let's Encrypt 集成，仅在 Leader Controller 上执行 |

## 一致性测试

Edgion 使用官方 `sigs.k8s.io/gateway-api` v1.4.0 提供的 Go 测试套件进行一致性测试。

### 运行方式

```bash
# 进入一致性测试目录
cd examples/gateway-api-conformance

# 运行一致性测试（需要 K8s 集群已部署 Edgion）
go test -v -run TestConformance ./...
```

### 测试覆盖

一致性测试验证以下核心行为：
- GatewayClass 和 Gateway 的创建与状态更新
- HTTPRoute 路由匹配（主机名、路径、Header）
- HTTPRoute 过滤器（RequestRedirect、RequestHeaderModifier、URLRewrite）
- GRPCRoute 路由匹配（Service、Method）
- 跨命名空间引用（ReferenceGrant）
- TLS 终止和 TLS passthrough
- BackendTLSPolicy 后端 mTLS

## 扩展点

Edgion 通过以下机制扩展标准 Gateway API：

### 1. 注解（Annotations）

使用 `edgion.io/*` 前缀的注解在标准资源上添加 Edgion 特定行为：

| 注解 | 适用资源 | 说明 |
|------|---------|------|
| `edgion.io/edgion-plugins` | HTTPRoute, GRPCRoute | 引用 EdgionPlugins 资源名称 |
| `edgion.io/edgion-stream-plugins` | Gateway | 引用 EdgionStreamPlugins 资源名称 |
| `edgion.io/edgion-gateway-config` | Gateway | 引用 EdgionGatewayConfig 资源名称 |
| `edgion.io/edgion-tls` | Gateway | 引用 EdgionTls 资源名称（替代 Secret） |
| `edgion.io/link-sys` | Gateway | 引用 LinkSys 资源名称 |

完整注解列表见 `skills/02-features/10-annotations/00-annotations-overview.md`。

### 2. ExtensionRef 过滤器

HTTPRoute 和 GRPCRoute 支持通过 `ExtensionRef` 类型的 filter 引用 Edgion 扩展资源：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-plugins
```

### 3. 自定义 CRD

上述 Edgion 扩展 CRD 表中列出的所有资源都是标准 Gateway API 之外的扩展。它们通过注解或 ExtensionRef 与标准资源关联，遵循 Gateway API 的扩展性设计原则。

## Gateway API 兼容性说明

更详细的兼容性说明、有意偏差和已知限制，请参见 `skills/08-gateway-api/SKILL.md`。
