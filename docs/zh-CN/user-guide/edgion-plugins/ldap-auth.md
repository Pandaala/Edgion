# LDAP Auth 插件

## 概述

LDAP Auth 插件用于将网关认证委托给企业 LDAP/AD 目录服务。插件从请求头解析 `username:password`，再使用 LDAP Simple Bind 校验凭证。

适用场景：
- 统一账号体系（LDAP / Active Directory）
- 不希望在网关内维护本地用户密码
- 需要与企业现有账号生命周期管理一致

## 功能特性

- 支持 `Authorization` / `Proxy-Authorization` 认证头
- `Proxy-Authorization` 优先级高于 `Authorization`
- 支持自定义认证方案名（`headerType`，如 `ldap` / `basic`）
- 支持匿名降级（`anonymous`）
- 支持隐藏凭证头（`hideCredentials`）
- 支持认证成功缓存（`cacheTtl`）
- 支持 LDAPS / StartTLS（通过 `ldaps` / `startTls`）

## 工作流程

1. 读取请求头：先读 `Proxy-Authorization`，再读 `Authorization`
2. 解析格式：`{headerType} base64(username:password)`
3. 可选缓存命中：命中则直接放行
4. 构建 Bind DN：
   - 默认：`{attribute}={username},{baseDn}`
   - 模板：`bindDnTemplate` 替换 `{username}`
5. LDAP Simple Bind 校验
6. 成功后注入上游头并继续转发

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `ldapHost` | String | 无 | LDAP 主机，必填 |
| `ldapPort` | Integer | `389` | LDAP 端口 |
| `ldaps` | Boolean | `false` | 是否启用 LDAPS |
| `startTls` | Boolean | `false` | 是否启用 StartTLS（与 `ldaps` 互斥） |
| `verifyLdapHost` | Boolean | `true` | 是否校验证书主机名 |
| `baseDn` | String | 无 | 基础 DN，必填 |
| `attribute` | String | 无 | 用户属性，必填（如 `uid`/`cn`） |
| `bindDnTemplate` | String | 无 | 自定义 Bind DN 模板，必须包含 `{username}` |
| `headerType` | String | `ldap` | 认证头方案名 |
| `hideCredentials` | Boolean | `false` | 是否移除认证头再转发 |
| `anonymous` | String | 无 | 匿名用户名（配置后无凭证可放行） |
| `realm` | String | `API Gateway` | `WWW-Authenticate` realm |
| `cacheTtl` | Integer | `60` | 认证缓存 TTL（秒），`0` 表示禁用缓存 |
| `timeout` | Integer | `10000` | LDAP 超时（毫秒） |
| `keepalive` | Integer | `60000` | 保留字段 |
| `credentialIdentifierHeader` | String | `X-Credential-Identifier` | 认证用户名透传头 |
| `anonymousHeader` | String | `X-Anonymous-Consumer` | 匿名标记头 |

## 最小配置示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        ldapPort: 389
        startTls: true
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        timeout: 3000
```

## 使用 `headerType: basic` 示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth-basic-scheme
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        headerType: "basic"
```

客户端可直接使用 Basic 认证：

```bash
curl -u alice:password123 https://api.example.com/protected
```

## 匿名降级示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth-anon
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        anonymous: "guest-user"
        hideCredentials: true
```

匿名放行时会注入：
- `X-Credential-Identifier: guest-user`
- `X-Anonymous-Consumer: true`

## 错误语义

| 场景 | 状态码 | 说明 |
|------|--------|------|
| 缺少凭证（且未启用 `anonymous`） | `401` | 返回 `WWW-Authenticate` |
| 凭证格式非法 | `401` | 通用认证失败 |
| LDAP 凭证错误 | `401` | 通用认证失败 |
| LDAP 服务不可达/超时 | `503` | 服务不可用 |

## 安全建议

- 生产环境优先使用 `ldaps: true` 或 `startTls: true`
- 保持 `verifyLdapHost: true`
- 开启 `hideCredentials: true`，避免凭证透传到上游
- 设置合理 `cacheTtl`（如 60~300 秒）平衡性能与凭证撤销生效时间
- 在 LDAP 服务器侧启用账户锁定与防爆破策略
