# Edgion 运维指南

本目录包含 Edgion Gateway 的平台级运维配置文档，面向运维工程师和平台管理员。

## 📚 指南列表

### Gateway 平台配置

- 🔄 **[HTTP to HTTPS 重定向](./http-to-https-redirect-guide.md)** - 通过 Gateway Annotation 配置全局重定向
- 📦 **[GatewayClass 配置](./gatewayclass-guide.md)** - GatewayClass 参数配置（即将推出）
- 🌐 **[Gateway 配置](./gateway-guide.md)** - Gateway 监听器和 Annotation 配置（即将推出）

### 启动配置

- ⚙️ **[Controller 配置](./controller-config-guide.md)** - edgion-controller.toml 配置说明（即将推出）
- 🚀 **[Gateway 启动配置](./gateway-config-guide.md)** - edgion-gateway.toml 配置说明（即将推出）

### Kubernetes 部署

- 📦 **[Kubernetes 部署指南](./kubernetes-deploy-guide.md)** - K8s 环境部署和配置（即将推出）
- 🔐 **[RBAC 配置](./rbac-guide.md)** - 权限和服务账号配置（即将推出）

### 监控与运维

- 📊 **[监控配置](./monitoring-guide.md)** - Prometheus/Grafana 集成（即将推出）
- 📝 **[日志管理](./logging-guide.md)** - 日志收集和分析（即将推出）

---

## 🔗 相关资源

- [用户指南](../user-guide/README.md) - 插件、路由、TLS 等功能配置
- [开发者文档](../developer-doc/README.md) - 架构和开发指南
- [示例配置](../../../examples/conf/) - 完整配置示例

---

## 📋 Gateway Annotation 快速参考

| Annotation | 类型 | 默认值 | 说明 |
|------------|------|--------|------|
| `edgion.com/enable-http2` | string | `"true"` | 控制 HTTP/2 支持 |
| `edgion.io/http-to-https-redirect` | string | `"false"` | 启用 HTTP→HTTPS 重定向 |
| `edgion.io/https-redirect-port` | string | `"443"` | HTTPS 重定向目标端口 |
| `edgion.io/backend-protocol` | string | - | TLS 监听器的后端协议 |

详细说明请参考 [Annotations 指南](../developer-doc/annotations-guide.md)。

---

## 📋 配置文件说明

### Controller 配置 (edgion-controller.toml)

```toml
work_dir = "/usr/local/edgion"
k8s_mode = true

[server]
grpc_listen = "0.0.0.0:50051"
admin_listen = "0.0.0.0:5800"
gateway_class = "public-gateway"
watch_namespaces = []  # 空 = 全部命名空间
# label_selector = "app.kubernetes.io/managed-by=edgion"
```

### Gateway 配置 (edgion-gateway.toml)

```toml
work_dir = "/usr/local/edgion"

[gateway]
server_addr = "http://edgion-controller:50051"

[logging]
log_level = "info"
json_format = true
```

---

**版本**: Edgion v0.1.0
