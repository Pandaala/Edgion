# Secret 管理

本文档说明 Edgion 在 TLS、认证插件、后端连接中对 Kubernetes Secret 的使用与运维建议。

## Secret 常见用途

- Gateway TLS 证书：`listeners[].tls.certificateRefs`
- 插件密钥：如 `jwt-auth`、`openid-connect`、`basic-auth`
- 后端认证材料：如 mTLS 客户端证书

## 管理建议

1. 按用途拆分 Secret，不要把无关凭据塞进一个对象。
2. 使用最小权限 RBAC，限制 Secret 可见范围。
3. 在生产环境启用 Secret 轮换策略。
4. 变更后观察资源状态与网关日志，确认已生效。

## 排障

### 症状：Gateway/Route 状态异常

- 检查 Secret 名称与命名空间是否正确。
- 检查 `ReferenceGrant` 是否允许跨命名空间引用。
- 检查 Secret key 名称是否与插件字段一致。

### 症状：TLS 握手失败

- 检查证书链与私钥是否匹配。
- 检查证书是否过期。
- 检查 SNI 与证书 SAN 是否匹配。

## 相关文档

- [TLS 终结](../gateway/tls/tls-termination.md)
- [EdgionTls 扩展](../gateway/tls/edgion-tls.md)
- [跨命名空间引用](./reference-grant.md)
