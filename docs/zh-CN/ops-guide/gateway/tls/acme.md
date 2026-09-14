# ACME 自动证书

> **🔌 Edgion 扩展**
>
> `EdgionAcme` 是 Edgion 自定义 CRD（`edgion.io/v1`），通过 ACME 协议（RFC 8555，如 Let's Encrypt）申请和续期 TLS 证书，并可自动创建/更新匹配的 EdgionTls 和 Secret。不属于标准 Gateway API 规范。

EdgionAcme 在控制器（Controller）侧运行 ACME 客户端：注册账号、验证域名所有权、签发和续期证书，将证书写入 Kubernetes Secret，并可选自动维护 EdgionTls 资源，供 Gateway 使用。

内置 ACME 仅支持 **HTTP-01**，不集成 DNS-01 或 DNS 提供商 API。需要 DNS-01 或通配符证书时，可用 cert-manager 等外部工具签发，再通过 Gateway/EdgionTls 引用其标准 TLS Secret；Edgion 已支持加载和热更新这些 Secret，无需创建 EdgionAcme。

## 概述

```
EdgionAcme (Controller) --ACME order--> ACME server (Let's Encrypt)
   ├─ HTTP-01: token published in status -> Gateway serves /.well-known/acme-challenge/<token>
   └─ on success: write cert to storage.secretName, update EdgionTls (autoEdgionTls)
```

**校验类型**

- **HTTP-01**：通过 `http://<domain>/.well-known/acme-challenge/<token>` 访问验证所有权。适合一个或多个非通配符域名；Gateway 的 HTTP 监听器必须可从公网访问。

**当前限制**

- 仅支持 **ECDSA** 证书（`ecdsa-p256` / `ecdsa-p384`）。
- 内置 ACME 独立管理账号和证书；也可直接加载 cert-manager 签发的 TLS Secret。

## 快速开始

### 前置条件

- Controller 已部署且具有读写 EdgionAcme、Secret 和 EdgionTls 的权限。
- HTTP-01 时：需要可访问的 HTTP 监听器（通常 80 端口），且域名解析到该 Gateway。

### 最小示例（HTTP-01，staging）

先使用 Let's Encrypt **staging** 验证流程——staging 的速率限制宽松，但签发的证书不受信任：

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: example-acme
  namespace: production
spec:
  # Let's Encrypt STAGING——仅用于测试（签发不受信任的证书）。
  server: "https://acme-staging-v02.api.letsencrypt.org/directory"
  email: admin@example.com
  privateKeySecretRef:
    name: example-acme-account
  domains:
    - example.com
    - www.example.com
  challenge:
    type: http-01
    gatewayRef:
      name: main-gateway
      namespace: production
  storage:
    secretName: example-com-tls
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: main-gateway
        sectionName: https
```

签发成功后，将 `server` 改为生产目录 `https://acme-v02.api.letsencrypt.org/directory`（省略 `server` 时默认使用）以获取受信任的证书。

创建后，Controller 注册 ACME 账号（将密钥存入 `privateKeySecretRef`），运行校验，将证书写入 `storage.secretName`，并创建/更新 EdgionTls。

## 配置参数

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `server` | String | Let's Encrypt 生产目录 | ACME 目录 URL，测试时使用 staging URL |
| `email` | String | **必填** | ACME 账号注册的联系邮箱 |
| `privateKeySecretRef` | Object | **必填** | 存储 ACME **账号**私钥的 Secret，不存在时 Edgion 自动创建；已有凭据时复用 |
| `domains` | Array\<String\> | **必填** | 申请的域名；不支持通配符域名 |
| `keyType` | Enum | `ecdsa-p256` | 证书密钥算法。可选值：`ecdsa-p256` \| `ecdsa-p384` |
| `challenge` | Object | **必填** | 校验配置；见[校验配置](#校验配置) |
| `renewal` | Object | （见下文默认值） | 续期时机；见[续期](#续期与重试renewal) |
| `externalAccountBinding` | Object | none | 部分 ACME 提供商（如 ZeroSSL）要求的外部账号绑定；见[外部账号绑定](#外部账号绑定) |
| `storage` | Object | **必填** | 已签发证书的存储位置；见[证书存储](#证书存储storage) |
| `autoEdgionTls` | Object | （已启用） | 自动创建/更新 EdgionTls；见[自动 EdgionTls](#自动-edgiontlsautoedgiontls) |

> `privateKeySecretRef`（ACME **账号**密钥）和 `storage.secretName`（签发的**证书**）是两个不同的 Secret。账号 Secret 在续期中复用；证书 Secret 每次签发时覆盖。

### 校验配置

`challenge` 仅接受 `type: http-01` 和必填的 `gatewayRef`，其他校验类型和未知字段会被拒绝。

**HTTP-01**（`type: http-01`）：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `gatewayRef` | Object | **必填** | 提供校验响应的 Gateway HTTP 监听器引用；可用 `namespace`、`sectionName` 和 `port` 缩小目标范围 |

只有被选中的明文 HTTP 监听器可以响应。若同时设置 `sectionName` 和
`port`，两者必须同时匹配。跨命名空间引用需要 `ReferenceGrant` 允许
`edgion.io/EdgionAcme` 引用 `gateway.networking.k8s.io/Gateway`；
`allowedRoutes` 不适用于 EdgionAcme。

Controller 将 token 发布到 `status.activeChallenges`；仅当校验、所选监听器和当前 Gateway 代次都匹配时，Gateway 才响应 `http://<domain>/.well-known/acme-challenge/<token>`，校验结束后停止拦截该路径。

### 续期与重试（renewal）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `renewal.renewBefore` | Duration | `720h` | 在证书到期前多久开始续期（720h = 30 天） |
| `renewal.checkInterval` | Duration | `24h` | 续期检查间隔 |
| `renewal.failBackoff` | Duration | `5m` | 失败后重试延迟的基数；每次连续失败乘以 4（`failBackoff * 4^attempt`，如 5m、20m、80m、约 5.3h、约 21.3h）。5 次连续失败后资源保持 `Failed` 状态，停止重试，直到编辑 CRD 或[手动触发](#手动触发)（重置计数器）；成功也会重置计数器 |

### 外部账号绑定

部分 ACME 提供商（如 ZeroSSL）要求外部账号绑定（EAB）以将新账号与已有账号关联：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `externalAccountBinding.keyId` | String | **必填（存在时）** | 提供商的 EAB 密钥标识符 |
| `externalAccountBinding.keySecretRef` | Object | **必填（存在时）** | 引用一个 Kubernetes Secret，其 `hmacKey` 数据条目存储提供商提供的 base64url 编码 HMAC 密钥 |

创建 Secret：

```bash
kubectl create secret generic eab-hmac-key \
  --from-literal=hmacKey="<提供商提供的-base64url-编码-hmac-密钥>" \
  -n production
```

```yaml
externalAccountBinding:
  keyId: "your-eab-key-id"
  keySecretRef:
    name: eab-hmac-key
    namespace: production
```

Let's Encrypt 不需要 EAB，完全省略 `externalAccountBinding` 即可。

### 证书存储（storage）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `storage.secretName` | String | **必填** | 存储已签发的 `tls.crt` 和 `tls.key` 的 Secret 名称 |
| `storage.secretNamespace` | String | EdgionAcme 所在命名空间 | 证书 Secret 的命名空间 |

### 自动 EdgionTls（autoEdgionTls）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `autoEdgionTls.enabled` | Boolean | `true` | 是否为已签发证书创建/更新 EdgionTls |
| `autoEdgionTls.name` | String | `acme-<EdgionAcme 名称>` | 自动创建的 EdgionTls 名称 |
| `autoEdgionTls.parentRefs` | Array\<Object\> | none | 自动创建的 EdgionTls 附加到的 Gateway 监听器 |

## 常见配置场景

### 1. 多域名证书（HTTP-01）

单个 EdgionAcme 可以在一张证书（SAN 列表）中覆盖多个非通配符域名：

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: multi-domain-acme
  namespace: production
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: multi-domain-acme-account
  domains:
    - example.com
    - www.example.com
    - api.example.com
  challenge:
    type: http-01
    gatewayRef:
      name: main-gateway
      namespace: production
  storage:
    secretName: multi-domain-tls
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: main-gateway
        sectionName: https
```

每个域名都必须解析到 Gateway 并通过各自的 HTTP-01 校验。

### 2. 生产签发（Let's Encrypt prod）

省略 `server` 默认使用生产目录。**注意速率限制**——先使用 staging 验证（见快速开始），再应用生产资源：

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: example-acme-prod
  namespace: production
spec:
  # 省略 server -> 默认使用 Let's Encrypt 生产目录。
  email: admin@example.com
  privateKeySecretRef:
    name: example-acme-account
  domains:
    - example.com
  challenge:
    type: http-01
    gatewayRef:
      name: main-gateway
      namespace: production
  storage:
    secretName: example-com-tls
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: main-gateway
        sectionName: https
```

## 手动触发

如需立即重新发起签发或续期（例如修复 DNS 或网络问题后），可触发重新处理——同时重置失败重试计数器。

### 方式一：注解资源

任意修改 metadata 都会使 Controller 重新处理该资源：

```bash
kubectl annotate edgionacme example-acme -n production \
  edgion.io/trigger="$(date +%s)" --overwrite
```

### 方式二：Admin API

> **需要认证**：所有 Controller Admin API 调用都需要 Bearer Token。将 `EDGION_TOKEN` 设置为 `edgion-cli gen-token` 生成的 token，或直接使用 `edgion-cli acme trigger`。详见[联邦 RBAC — CLI Token RBAC](../../federation-rbac.md#cli-token-rbac)。

```bash
curl -X POST "http://<controller-admin>:12101/api/v1/services/acme/<namespace>/<name>/trigger" \
  -H "Authorization: Bearer $EDGION_TOKEN"
```

成功响应：

```json
{
  "success": true,
  "data": "ACME check triggered for production/example-acme"
}
```

如果 ACME 服务未在被访问的副本上运行（例如未持有 leadership 的副本），请求不会入队，响应会如实报告失败：

```json
{
  "success": false,
  "error": "ACME service not running; trigger for production/example-acme was not queued"
}
```

## 状态与排查

### 状态阶段（status.phase）

| 阶段 | 说明 |
|------|------|
| `Pending` | 尚未签发证书——等待首次签发或重试 |
| `Issuing` | ACME 订单进行中（校验或签发中） |
| `Ready` | 证书已签发且有效 |
| `Renewing` | 续期进行中 |
| `Failed` | 最近一次操作失败；使用指数退避重试（`failBackoff * 4^attempt`），最多 5 次，之后保持 `Failed` 状态，直到编辑 CRD 或[手动触发](#手动触发)重置重试计数器 |

### 常用状态字段

- `status.certificateSerial`、`status.certificateNotAfter`——当前证书的序列号和到期时间（RFC 3339）
- `status.lastFailureReason`、`status.lastFailureTime`——最近一次失败的详情
- `status.secretName`、`status.edgionTlsName`——当前使用的 Secret 和 EdgionTls
- `status.accountUri`——已注册的 ACME 账号 URI

### 常见问题

1. **HTTP-01 失败**——确认域名已解析到 Gateway 且其 HTTP 端口（通常 80）可从公网访问。Gateway 必须通过 List-Watch 接收到已发布的 token 并响应 `/.well-known/acme-challenge/<token>`。
2. **长期处于 `Failed`**——查看 `status.lastFailureReason`，修复原因后使用[手动触发](#手动触发)重新发起并重置重试计数器。
3. **通配符请求被拒绝**——内置 HTTP-01 不支持通配符；请使用 cert-manager 等外部工具签发，并引用生成的 TLS Secret。
4. **被 ACME 服务器限速**——测试时切换到 staging 目录；Let's Encrypt 生产环境有严格的每域名限制。

## 获取帮助

如遇问题：

1. 检查上方的状态与排查部分
2. 检查 Controller 日志（ACME 在 leader Controller 上运行）
3. 在 GitHub 提交 issue

## 相关文档

- [EdgionTls 扩展](./edgion-tls.md)
- [TLS 终结](./tls-termination.md)
- [HTTPS 监听器](../listeners/https.md)
- [联邦 RBAC](../../federation-rbac.md)
