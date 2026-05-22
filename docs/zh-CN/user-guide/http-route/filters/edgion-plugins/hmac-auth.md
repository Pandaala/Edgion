# HMAC Auth 插件

> **🔌 Edgion 扩展**
> 
> HmacAuth 是 `EdgionPlugins` CRD 提供的 HMAC 签名认证插件，不属于标准 Gateway API。

## 概述

HMAC Auth 基于 HTTP Signature 规范，通过 HMAC 算法验证请求签名，确保请求的完整性和身份真实性。支持 hmac-sha256、hmac-sha384、hmac-sha512 算法。

**工作原理**：
1. 客户端使用共享密钥对请求的指定部分（header、path 等）计算 HMAC 签名
2. 将签名信息放入 `Authorization` 或 `Signature` header
3. 插件使用相同的密钥重新计算签名并与请求中的签名比对
4. 签名匹配且在时间窗口内：允许访问
5. 签名不匹配或过期：返回 401

## 快速开始

### 创建凭证 Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: hmac-credentials
type: Opaque
stringData:
  username: "service-a"
  secret: "a-very-long-secret-key-at-least-32-chars!"
```

### 配置插件

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: hmac-auth-plugin
spec:
  requestPlugins:
    - enable: true
      type: HmacAuth
      config:
        secretGroups:
          - name: default
            secretRefs:
              - name: hmac-credentials
        algorithms:
          - hmac-sha256
        clockSkew: 300
```

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `secretGroups` | Array | ✅* | 无 | 凭证 Secret 分组列表（见下方子表） |
| `algorithms` | Array | ❌ | `["hmac-sha256","hmac-sha384","hmac-sha512"]` | 允许的签名算法 |
| `clockSkew` | Integer | ❌ | `300` | 允许的时钟偏移秒数 |
| `enforceHeaders` | Array | ❌ | 无 | 签名中必须包含的 header 列表 |
| `validateRequestBody` | Boolean | ❌ | `false` | 是否验证请求体的 Digest |
| `hideCredentials` | Boolean | ❌ | `false` | 是否移除认证相关 header |
| `anonymous` | String | ❌ | 无 | 匿名用户名 |
| `realm` | String | ❌ | `"edgion"` | 认证域 |
| `authFailureDelayMs` | Integer | ❌ | `0` | 认证失败延迟响应毫秒数 |
| `minSecretLength` | Integer | ❌ | `32` | 最小密钥长度 |
| `secretField` | String | ❌ | `"secret"` | Secret 中密钥的字段名 |
| `usernameField` | String | ❌ | `"username"` | Secret 中用户名的字段名 |
| `upstreamHeaderFields` | Array | ❌ | `[]` | 传递给上游的额外 header |

\* 未设置 `anonymous` 时必填。

### `secretGroups` 子表（`HmacAuthSecretGroup`）

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | ✅ | 组名（Kubernetes label-value 字符集，≤63 字符） |
| `description` | String | ❌ | 人工可读描述（运行时不使用） |
| `secretRefs` | Array | ✅ | 该组的 Kubernetes Secret 引用列表（至少 1 个） |

---

## 常见配置场景

### 场景 1：基础 HMAC 认证

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretGroups:
        - name: default
          secretRefs:
            - name: hmac-credentials
      algorithms:
        - hmac-sha256
      hideCredentials: true
```

### 场景 2：强制签名特定 Header

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretGroups:
        - name: default
          secretRefs:
            - name: hmac-credentials
      enforceHeaders:
        - "@request-target"
        - host
        - date
        - content-type
      validateRequestBody: true
```

### 场景 3：多用户认证

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretGroups:
        - name: default
          secretRefs:
            - name: user-a-credentials
            - name: user-b-credentials
      algorithms:
        - hmac-sha256
        - hmac-sha512
      clockSkew: 600
      upstreamHeaderFields:
        - X-Consumer-Username
```

---

## 签名格式

客户端请求需包含以下 Header：

```
Authorization: Signature keyId="service-a",algorithm="hmac-sha256",headers="@request-target host date",signature="base64-encoded-signature"
Date: Mon, 08 Mar 2026 10:00:00 GMT
```

### 签名计算步骤

1. 构建签名字符串：按 `headers` 参数指定的顺序拼接各 header 的值
2. 使用 HMAC 算法和共享密钥计算签名
3. Base64 编码签名结果

---

## 防重放安全模型

HMAC Auth 插件使用**时间戳窗口**机制防止签名重放——请求 `Date` / `X-Date` header 与服务器时钟差必须 ≤ `clockSkew`（默认 `300` 秒 / 5 分钟）。**插件本身不做 nonce 去重**。

这与业界主流 HMAC 认证实现一致（AWS SigV4、Stripe Webhooks、GitHub Webhooks、Kong `hmac-auth`、APISIX `hmac-auth`）——都是"时间戳窗口 + HTTPS"组合，不在代理层做 nonce 存储。

### 窗口内重放风险与缓解

理论上，攻击者若获得有效签名，可在 `clockSkew` 窗口内无限重放。该威胁的**前提**是拿到签名；在正确部署下有多层阻断：

| 威胁路径 | 防御 |
|---|---|
| 中间人抓包 | **HTTPS**（TLS 阻断签名泄露） |
| 签名从日志泄露 | 启用 `hideCredentials: true`；access log 不记录 `Authorization` / `Signature` 头 |
| 合法客户端恶意重放 | **业务层 idempotency key**（非网关层职责） |
| 窗口期调得过大 | 高敏感接口缩短 `clockSkew`（如 `60`） |

### 推荐配置

**生产环境最小安全配置**：

```yaml
filters:
  - type: HmacAuth
    config:
      secretGroups:
        - name: default
          secretRefs:
            - name: hmac-creds
      clockSkew: 300          # 5 分钟窗口；高敏感接口建议缩到 60
      hideCredentials: true   # 不把 Authorization/Signature 透传给上游（降低日志泄漏面）
      enforceHeaders:
        - date                # 时间戳必签
        - host
      # 业务层 idempotency：高敏感接口（支付、改密）必须由后端实现
```

**同时要求**：
- **必须走 HTTPS**：HMAC 签名在明文下可被中间人捕获，HTTPS 是本插件的基础假设
- **不要把签名写进 access log**：默认不写，但注意不要在 `accessLog.extraHeaders` 里加入 `Authorization` / `Signature`
- **关键操作业务层做 idempotency**：对 "非幂等 + 高敏感" 的操作（支付、改密、转账），业务端应要求 `Idempotency-Key` 请求头，后端在一段时间内对同 key 只执行一次——这是防重放的正确分层

### 为什么 Edgion 不在网关层做 nonce 去重？

做 nonce dedup 需要**请求级状态存储**（内存 LRU 或 Redis），两种方案都有明显问题：

- **内存 LRU**：多 gateway 实例部署时每实例独立，攻击者切换实例就能重放——变成**假象防御**
- **Redis 后端**：每请求 1 次 Redis 往返（~1–3ms 延迟），引入强依赖；Redis 故障时 fail-open 还是 fail-close 都是取舍困境

业界（AWS / Stripe / GitHub / Kong / APISIX）都选择"网关做时间戳窗口、业务层做 idempotency key"的分层，Edgion 遵循同一模式。

若你的部署形态确实需要网关层 nonce dedup（极端情况），可提 issue 讨论独立实现方案。

---

## 按组组织 Secret

`secretGroups` 允许将多个合作伙伴或租户的凭证分组管理，每个组独立维护 Secret 引用，并在访问日志中携带组名标识（`OK group=<name> user <X>`），便于审计归因。

**跨组用户名冲突处理**：同一用户名在多个组中出现时，遵循**先到先得（first-wins）**规则——第一个组的凭证被保留，后续重复条目产生 `warn` 日志但不影响插件整体可用性。

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretGroups:
        - name: partner-a
          description: "Partner A API credentials"
          secretRefs:
            - name: hmac-credentials-partner-a
              namespace: edgion-default
        - name: partner-b
          description: "Partner B API credentials"
          secretRefs:
            - name: hmac-credentials-partner-b
              namespace: edgion-default
      algorithms:
        - hmac-sha256
      clockSkew: 300
      enforceHeaders:
        - "@request-target"
        - host
        - date
      upstreamHeaderFields:
        - X-Consumer-Username
        - X-Customer-ID
```

认证成功时，访问日志记录：

```
OK group=partner-a user partner-a-user
OK group=partner-b user partner-b-user
```

---

## 故障排除

### 问题 1：签名验证失败

**原因**：时钟偏移过大或签名计算错误。

**解决方案**：
- 确保客户端和服务端时间同步
- 增大 `clockSkew` 值
- 检查签名字符串的构造是否正确

### 问题 2：密钥长度不足

**原因**：Secret 中的密钥短于 `minSecretLength`。

**解决方案**：使用至少 32 字符的密钥，或调整 `minSecretLength`。

---

## 相关文档

- [Basic Auth](./basic-auth.md)
- [Key Auth](./key-auth.md)
- [JWT Auth](./jwt-auth.md)
