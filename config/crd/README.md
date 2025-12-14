# Kubernetes Gateway API CRDs

此目录包含 Kubernetes Gateway API 的 CustomResourceDefinition (CRD) 文件。

## 版本信息

- **Gateway API Version**: v1.4.0
- **Channel**: Standard
- **下载日期**: 2025-12-14
- **来源**: [kubernetes-sigs/gateway-api](https://github.com/kubernetes-sigs/gateway-api/releases/tag/v1.4.0)

## 包含的 CRDs

### 标准版 (Standard Channel) - `gateway-api-standard-v1.4.0.yaml`

此文件包含以下 6 个标准 Gateway API 资源：

1. **BackendTLSPolicy** (`backendtlspolicies.gateway.networking.k8s.io`)
   - 配置 Gateway 如何通过 TLS 连接到 Backend

2. **GatewayClass** (`gatewayclasses.gateway.networking.k8s.io`)
   - 定义一组具有共同配置和行为的 Gateway

3. **Gateway** (`gateways.gateway.networking.k8s.io`)
   - 描述如何将流量转换为集群内服务的请求

4. **GRPCRoute** (`grpcroutes.gateway.networking.k8s.io`)
   - 为 gRPC 流量定义路由规则

5. **HTTPRoute** (`httproutes.gateway.networking.k8s.io`)
   - 为 HTTP/HTTPS 流量定义路由规则

6. **ReferenceGrant** (`referencegrants.gateway.networking.k8s.io`)
   - 允许跨命名空间的资源引用

## 使用说明

### 部署到 Kubernetes 集群

```bash
# 安装标准版 Gateway API CRDs
kubectl apply -f config/crd/gateway-api-standard-v1.4.0.yaml

# 验证安装
kubectl get crds | grep gateway.networking.k8s.io
```

### 查看已安装的 CRDs

```bash
# 查看所有 Gateway API CRDs
kubectl get crds -l gateway.networking.k8s.io/channel=standard

# 查看特定 CRD 的详细信息
kubectl describe crd gateways.gateway.networking.k8s.io
```

### 卸载

```bash
kubectl delete -f config/crd/gateway-api-standard-v1.4.0.yaml
```

## Edgion 支持状态

Edgion 目前支持以下 Gateway API 资源：

- ✅ **HTTPRoute** - 完全支持
- ✅ **GRPCRoute** - 完全支持
- ✅ **TCPRoute** - 完全支持 (实验性)
- ✅ **UDPRoute** - 完全支持 (实验性)
- ✅ **TLSRoute** - 完全支持 (实验性)
- ✅ **Gateway** - 完全支持
- ✅ **GatewayClass** - 完全支持
- ⚠️  **BackendTLSPolicy** - 计划中
- ⚠️  **ReferenceGrant** - 计划中

## 其他渠道

Gateway API 还提供其他安装渠道：

- **Experimental Channel**: 包含实验性功能（如 TCPRoute, UDPRoute, TLSRoute）
- **Standard Channel**: 仅包含稳定的标准功能（当前文件）

## 参考链接

- [Gateway API 官方文档](https://gateway-api.sigs.k8s.io/)
- [Gateway API v1.4.0 发布说明](https://github.com/kubernetes-sigs/gateway-api/releases/tag/v1.4.0)
- [Gateway API GitHub 仓库](https://github.com/kubernetes-sigs/gateway-api)
- [API 参考文档](https://gateway-api.sigs.k8s.io/reference/spec/)

## 更新 CRDs

要更新到最新版本：

```bash
# 下载最新版本 (检查最新版本号)
curl -L -o config/crd/gateway-api-standard-v1.4.0.yaml \
  https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.4.0/standard-install.yaml

# 应用更新
kubectl apply -f config/crd/gateway-api-standard-v1.4.0.yaml
```

## 许可证

Gateway API 使用 Apache License 2.0 许可证。
详见文件头部的版权声明。

