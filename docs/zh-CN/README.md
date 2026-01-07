# Edgion 文档

欢迎使用 Edgion - 一个基于 Pingora 和 Kubernetes Gateway API 的高性能网关。

## 📖 文档导航

### 🚀 [用户指南](./user-guide/README.md)

快速上手和功能使用教程：
- [Stream Plugins 使用指南](./user-guide/stream-plugins-guide.md) - TCP/UDP 流式插件
- [CORS 跨域配置](./user-guide/cors-user-guide.md) - 跨域资源共享

### 🔧 [运维指南](./op-guide/README.md)

Gateway 平台级运维配置：
- [HTTP to HTTPS 重定向](./op-guide/http-to-https-redirect-guide.md) - Gateway Annotation 配置
- [启动配置文件](./op-guide/README.md#配置文件说明) - Controller/Gateway toml 配置

### 🛠️ [开发者文档](./developer-doc/README.md)

架构设计和开发指南：
- [架构概览](./developer-doc/architecture-overview.md)
- [添加新资源类型](./developer-doc/add-new-resource-guide.md)
- [Annotations 参考](./developer-doc/annotations-guide.md)

---

## 🎯 快速链接

| 类型 | 链接 | 说明 |
|------|------|------|
| 📚 用户指南 | [user-guide/](./user-guide/) | 插件和功能使用 |
| 🔧 运维指南 | [op-guide/](./op-guide/) | Gateway/TLS 配置 |
| 🛠️ 开发文档 | [developer-doc/](./developer-doc/) | 架构和开发 |
| 📦 示例配置 | [examples/conf/](../../examples/conf/) | YAML 配置示例 |
| 🧪 测试 | [examples/testing/](../../examples/testing/) | 集成测试 |

---

## 🌟 特性

- ✅ 支持 Gateway API v1
- ✅ TCP/UDP 流式插件系统
- ✅ mTLS 双向认证
- ✅ 动态证书加载（SNI-based）
- ✅ IP 访问控制
- ✅ 基于 Pingora 的高性能代理

---

## 📝 版本

**当前版本**: v0.1.0  
**最后更新**: 2025-12-25

