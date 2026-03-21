---
name: http-plugin-catalog
description: EdgionPlugins CRD Schema 和 28 个 HTTP 插件完整目录。
---

# EdgionPlugins — HTTP 插件

> API: `edgion.io/v1` | Scope: Namespaced

EdgionPlugins 资源定义一组 HTTP 层插件配置，通过 ExtensionRef 被路由引用。

## 基础 Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-plugins
  namespace: default
spec:
  plugins:
    - type: RateLimit           # 插件类型
      config:                   # 插件配置（因类型而异）
        rate: 100
        burst: 200
        key: "$remote_addr"
    - type: Cors
      config:
        allowOrigins: ["*"]
```

## 插件目录

### 认证类

| 类型 | 说明 | Secret 依赖 |
|------|------|------------|
| `BasicAuth` | HTTP Basic 认证 | ✅ username/password |
| `JwtAuth` | JWT Token 验证（RSA/EC/HMAC/EdDSA） | ✅ 公钥/密钥 |
| `JweDecrypt` | JWE 加密 Token 解密 | ✅ 加密密钥 |
| `HmacAuth` | HMAC 签名验证 | ✅ access_key + secret_key |
| `KeyAuth` | API Key 认证 | ✅ API keys |
| `HeaderCertAuth` | 请求头/连接证书认证 | — |
| `LdapAuth` | LDAP 认证 | — |
| `ForwardAuth` | 外部认证服务转发 | — |
| `OpenidConnect` | OpenID Connect / OAuth2 | ✅ client_secret + session_secret |

### 安全类

| 类型 | 说明 |
|------|------|
| `Cors` | CORS 跨域资源共享 |
| `Csrf` | CSRF 防护 |
| `IpRestriction` | IP 黑白名单（支持 CIDR） |
| `RequestRestriction` | 请求限制（header/body 规则） |

### 流量控制类

| 类型 | 说明 |
|------|------|
| `RateLimit` | 本地限流（CMS 算法） |
| `RateLimitRedis` | 分布式限流（Redis 后端） |
| `BandwidthLimit` | 带宽限制 |

### 请求/响应修改类

| 类型 | 说明 |
|------|------|
| `RequestHeaderModifier` | 修改请求头（Gateway API 标准） |
| `ResponseHeaderModifier` | 修改响应头（Gateway API 标准） |
| `RequestRedirect` | HTTP 重定向（Gateway API 标准） |
| `UrlRewrite` | URL 重写（Gateway API 标准） |
| `RequestMirror` | 请求镜像（Gateway API 标准） |
| `ProxyRewrite` | 代理重写（高级路径/头部重写） |
| `ResponseRewrite` | 响应重写（状态码/头部/Body） |
| `RealIp` | 真实 IP 提取 |
| `CtxSet` | 设置上下文变量 |

### 路由/后端类

| 类型 | 说明 |
|------|------|
| `ExtensionRef` | 嵌套引用其他 EdgionPlugins |
| `DirectEndpoint` | 直接指定后端端点 |
| `DynamicInternalUpstream` | 动态内部上游 |
| `DynamicExternalUpstream` | 动态外部上游 |
| `AllEndpointStatus` | 所有端点状态查询 |

### 调试/测试类

| 类型 | 说明 |
|------|------|
| `Mock` | Mock 响应（测试/原型） |
| `DebugAccessLogToHeader` | 调试用：Access Log 注入响应头 |
| `Dsl` | DSL 脚本插件（沙箱 VM 执行） |

## 插件执行阶段

HTTP 插件在 Pingora 的 4 个阶段执行：

| 阶段 | 触发时机 | 典型插件 |
|------|---------|---------|
| `RequestFilter` | 收到请求后、选择后端前 | Auth、RateLimit、CORS、IP 限制 |
| `UpstreamResponseFilter` | 收到后端响应头后 | ResponseHeaderModifier |
| `UpstreamResponseBodyFilter` | 收到后端响应 Body 后 | ResponseRewrite（Body 修改） |
| `UpstreamResponse` | 后端响应完成后 | 日志记录 |

## Secret 依赖

以下插件类型在 Controller 侧 `parse()` 阶段会从 `GLOBAL_SECRET_STORE` 读取 Secret 数据：

| 插件 | 读取的 Secret 字段 |
|------|-------------------|
| `BasicAuth` | username, password |
| `JwtAuth` | RSA/EC/HMAC/EdDSA public keys, key metadata |
| `JweDecrypt` | JWE encryption key |
| `HmacAuth` | access_key, secret_key |
| `KeyAuth` | API keys |
| `OpenidConnect` | client_secret, session_secret |
| `HeaderCertAuth` | CA certificates |

Secret 变更会通过 SecretRefManager 自动触发 EdgionPlugins 重新处理。
