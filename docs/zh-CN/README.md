# Edgion 文档

欢迎使用 Edgion - 一个基于 Pingora 和 Kubernetes Gateway API 的高性能网关。

## 📖 文档导航

### 🚀 [快速开始](./getting-started/README.md)

安装部署和快速入门。

### 📚 [用户指南](./user-guide/README.md)

面向应用开发者的路由和插件配置：
- HTTPRoute / GRPCRoute / TCPRoute / UDPRoute
- Edgion 扩展插件（BasicAuth、CORS、CSRF、IP限制等）
- 负载均衡算法

### 🔧 [运维指南](./ops-guide/README.md)

面向运维人员的 Gateway 配置：
- Gateway / GatewayClass 配置
- TLS / mTLS 配置
- 访问日志和监控

### 🛠️ [开发指南](./dev-guide/README.md)

面向开发者的架构和扩展开发：
- [架构概览](./dev-guide/architecture-overview.md)
- [添加新资源类型](./dev-guide/add-new-resource-guide.md)
- [Annotations 参考](./dev-guide/annotations-guide.md)

---

## 🎯 快速链接

| 类型 | 链接 | 说明 |
|------|------|------|
| 📚 用户指南 | [user-guide/](./user-guide/) | 路由和插件配置 |
| 🔧 运维指南 | [ops-guide/](./ops-guide/) | Gateway/TLS 配置 |
| 🛠️ 开发指南 | [dev-guide/](./dev-guide/) | 架构和开发 |
| 📦 示例配置 | [examples/k8stest/](../../examples/k8stest/) | 集成测试与示例配置 |

---

## 📝 版本

**当前版本**: v0.1.0  
**最后更新**: 2026-01-09
