# Edgion 用户指南

欢迎使用 Edgion！这里提供快速上手和常用功能的使用指南。

## 📚 指南列表

### 安全认证

- 🔐 **[Basic Auth 认证](./basic-auth-user-guide.md)** - HTTP 基础认证配置
- 🛡️ **[CSRF 防护](./csrf-user-guide.md)** - 跨站请求伪造防护
- 🚫 **[IP 限制](./ip-restriction.md)** - IP 黑白名单配置

### 跨域与 TLS

- 🌐 **[CORS 跨域](./cors-user-guide.md)** - 跨域资源共享配置
- ✈️ **[Preflight 策略](./preflight-policy-guide.md)** - 预检请求处理配置
- 🔒 **[EdgionTLS](./edgiontls-user-guide.md)** - TLS 证书管理

### 流式处理

- 🌊 **[Stream Plugins](./stream-plugins-guide.md)** - TCP/UDP 流式插件
  - IP 访问控制
  - 跨命名空间引用
  - 常见使用场景

### 监控与日志

- 📊 **[Access Log](./access-log-guide.md)** - 访问日志格式和分析
  - JSON 日志格式说明
  - 插件日志解读
  - 日志分析最佳实践

### 即将推出

- 🔐 mTLS 配置指南
- 🚀 性能优化指南

---

## 🔗 其他资源

### 开发者文档
详细的架构和开发指南，请参考 [Developer Documentation](../developer-doc/README.md)

### 示例配置
查看 `examples/conf/` 目录获取完整的配置示例

### 测试
查看 `examples/testing/` 目录了解如何运行测试

---

## 💬 获取帮助

遇到问题？
- 📖 查看 [Annotations 参考](../developer-doc/annotations-guide.md)
- 🐛 [提交 Issue](https://github.com/your-org/edgion/issues)
- 💡 [查看示例](../../examples/conf/)

---

**版本**: Edgion v0.1.0
