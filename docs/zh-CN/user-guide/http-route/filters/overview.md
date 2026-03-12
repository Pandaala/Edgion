# 过滤器总览

过滤器（Filters）用于在请求路由过程中修改请求或响应。

## 过滤器类型

### Gateway API 标准过滤器

| 类型 | 说明 | 文档 |
|------|------|------|
| RequestHeaderModifier | 修改请求头 | [详情](./gateway-api/request-header-modifier.md) |
| ResponseHeaderModifier | 修改响应头 | [详情](./gateway-api/response-header-modifier.md) |
| RequestRedirect | 请求重定向 | [详情](./gateway-api/request-redirect.md) |
| URLRewrite | URL 重写 | [详情](./gateway-api/url-rewrite.md) |
| RequestMirror | 请求镜像 | 即将推出 |

### Edgion 扩展过滤器

> **🔌 Edgion 扩展**
> 
> 以下插件通过 `EdgionPlugins` CRD 实现，是 Edgion 的扩展功能。

通过 `ExtensionRef` 引用 EdgionPlugins 资源：

| 插件 | 说明 | 文档 |
|------|------|------|
| BasicAuth | HTTP 基础认证 | [详情](./edgion-plugins/basic-auth.md) |
| LdapAuth | LDAP 目录认证 | [详情](../../edgion-plugins/ldap-auth.md) |
| CORS | 跨域资源共享 | [详情](./edgion-plugins/cors.md) |
| CSRF | CSRF 防护 | [详情](./edgion-plugins/csrf.md) |
| IpRestriction | IP 黑白名单 | [详情](./edgion-plugins/ip-restriction.md) |
| RateLimit | 限流 | [详情](./edgion-plugins/rate-limit.md) |

## 过滤器执行顺序

```
请求 → RequestHeaderModifier → ExtensionRef(插件) → URLRewrite → 后端
后端 → ResponseHeaderModifier → 响应
```

## 配置示例

### 使用标准过滤器

```yaml
filters:
  - type: RequestHeaderModifier
    requestHeaderModifier:
      add:
        - name: X-Gateway
          value: edgion
```

### 使用 Edgion 插件

```yaml
filters:
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: EdgionPlugins
      name: my-cors-plugin
```

## 相关文档

- [HTTPRoute 总览](../overview.md)
- [后端配置](../backends/README.md)
