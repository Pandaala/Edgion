# ACME 自动证书

> **🔌 Edgion 扩展**
>
> `EdgionAcme` 是 Edgion 自定义 CRD（`edgion.io/v1`），通过 ACME 协议（RFC 8555，如 Let's Encrypt）申请和续期 TLS 证书，并可自动创建/更新匹配的 EdgionTls 和 Secret。不属于标准 Gateway API 规范。

EdgionAcme 在控制器（Controller）侧运行 ACME 客户端：注册账号、验证域名所有权、签发和续期证书，将证书写入 Kubernetes Secret，并可选自动维护 EdgionTls 资源，供 Gateway 使用。

## 概述

```
EdgionAcme (Controller) --ACME order--> ACME server (Let's Encrypt)
   ├─ HTTP-01: token published in status -> Gateway serves /.well-known/acme-challenge/<token>
   ├─ DNS-01:  _acme-challenge TXT record created via provider API
   └─ on success: write cert to storage.secretName, update EdgionTls (autoEdgionTls)
```

**校验类型**

- **HTTP-01**：通过 `http://<domain>/.well-known/acme-challenge/<token>` 访问验证所有权。适合单域名或非通配符；Gateway 的 HTTP 监听器必须可从公网访问。
- **DNS-01**：通过 DNS TXT 记录验证，是通配符域名（如 `*.example.com`）的**唯一**选项。

**当前限制**

- 仅支持 **ECDSA** 证书（`ecdsa-p256` / `ecdsa-p384`）。
- 不与 cert-manager 互通——Edgion 独立管理 ACME 账号和证书。

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
| `domains` | Array\<String\> | **必填** | 申请的域名；含通配符（如 `*.example.com`）时必须使用 DNS-01 |
| `keyType` | Enum | `ecdsa-p256` | 证书密钥算法。可选值：`ecdsa-p256` \| `ecdsa-p384` |
| `challenge` | Object | **必填** | 校验配置；见[校验配置](#校验配置) |
| `renewal` | Object | （见下文默认值） | 续期时机；见[续期](#续期与重试renewal) |
| `externalAccountBinding` | Object | none | 部分 ACME 提供商（如 ZeroSSL）要求的外部账号绑定；见[外部账号绑定](#外部账号绑定) |
| `storage` | Object | **必填** | 已签发证书的存储位置；见[证书存储](#证书存储storage) |
| `autoEdgionTls` | Object | （已启用） | 自动创建/更新 EdgionTls；见[自动 EdgionTls](#自动-edgiontlsautoedgiontls) |

> `privateKeySecretRef`（ACME **账号**密钥）和 `storage.secretName`（签发的**证书**）是两个不同的 Secret。账号 Secret 在续期中复用；证书 Secret 每次签发时覆盖。

### 校验配置

`challenge` 是以 `type` 为键的标签枚举；类型特定字段直接平铺在 `challenge` 下（没有 `http01:` / `dns01:` 子对象）。

**HTTP-01**（`type: http-01`）：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `gatewayRef` | Object | **必填** | 提供校验响应的 Gateway 的 HTTP 监听器引用 |

Controller 将 token 发布到 `status.activeChallenges`；Gateway 读取后仅在校验进行中时响应 `http://<domain>/.well-known/acme-challenge/<token>`，校验结束后停止拦截该路径。

**DNS-01**（`type: dns-01`）：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `provider` | String | **必填** | DNS 提供商。可选值：`cloudflare` \| `alidns` |
| `credentialRef` | Object | **必填** | 持有提供商 API 凭据的 Secret 引用 |
| `propagationTimeout` | Integer | `120` | 等待 TXT 记录传播的最长时间（秒） |
| `propagationCheckInterval` | Integer | `5` | 传播检查间隔（秒） |

Controller 通过提供商 API 创建 `_acme-challenge.<domain>` TXT 记录，等待传播后请求 ACME 验证，验证后删除记录。提供商凭据 Secret 键：

- **Cloudflare**——`api-token`（需要 Zone.DNS:Edit 权限）
- **阿里云 DNS（alidns）**——`access-key-id` 和 `access-key-secret`

### 续期与重试（renewal）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `renewal.renewBeforeDays` | Integer | `30` | 在证书到期前多少天开始续期 |
| `renewal.checkInterval` | Integer | `86400` | 续期检查间隔（秒，默认 24h） |
| `renewal.failBackoff` | Integer | `300` | 失败后重试延迟的基数（秒）；每次连续失败乘以 4（`failBackoff * 4^attempt`，如 300、1200、4800、19200、76800）。5 次连续失败后资源保持 `Failed` 状态，停止重试，直到编辑 CRD 或[手动触发](#手动触发)（重置计数器）；成功也会重置计数器 |

### 外部账号绑定

部分 ACME 提供商（如 ZeroSSL）要求外部账号绑定（EAB）以将新账号与已有账号关联：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `externalAccountBinding.keyId` | String | **必填（存在时）** | 提供商的 EAB 密钥标识符 |
| `externalAccountBinding.hmacKey` | String | **必填（存在时）** | 提供商的 base64url 编码 HMAC 密钥 |

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

### 1. 通配符证书（DNS-01，Cloudflare）

通配符证书只能使用 DNS-01 签发：

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: wildcard-acme
  namespace: production
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: wildcard-acme-account
  domains:
    - "*.example.com"
    - example.com
  keyType: ecdsa-p384
  challenge:
    type: dns-01
    provider: cloudflare
    credentialRef:
      name: cloudflare-api-token
      namespace: production
    propagationTimeout: 180
  renewal:
    renewBeforeDays: 14
    checkInterval: 43200
  storage:
    secretName: wildcard-example-com-tls
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: main-gateway
        sectionName: https
```

创建提供商凭据 Secret：

```bash
kubectl create secret generic cloudflare-api-token \
  --from-literal=api-token=<cloudflare-api-token> \
  -n production
```

⚠️ `<cloudflare-api-token>` 是占位符——切勿提交真实 token。

### 2. 多域名证书（HTTP-01）

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

### 3. 生产签发（Let's Encrypt prod）

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
2. **DNS-01 超时**——增大 `propagationTimeout`；确认 `credentialRef` Secret 键正确且提供商 API 可访问。
3. **长期处于 `Failed`**——查看 `status.lastFailureReason`，修复原因后使用[手动触发](#手动触发)重新发起并重置重试计数器。
4. **通配符请求被拒绝**——通配符必须使用 `challenge.type: dns-01`，HTTP-01 无法验证通配符。
5. **被 ACME 服务器限速**——测试时切换到 staging 目录；Let's Encrypt 生产环境有严格的每域名限制。

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

- **HTTP-01**：通过 HTTP 访问 `/.well-known/acme-challenge/<token>` 完成校验，适合单域名或非通配符。
- **DNS-01**：通过 DNS TXT 记录完成校验，**支持通配符域名**（如 `*.example.com`）。
- **自动续期**：按配置在到期前自动续期，失败时指数退避重试（最多 5 次）。
- **与 EdgionTls 联动**：签发后可自动创建或更新 EdgionTls，无需手工维护证书资源。

**当前限制**：

- 仅支持 **ECDSA** 证书（ecdsa-p256 / ecdsa-p384）。
- 不与 Kubernetes cert-manager 互通；证书与账号由 Edgion Controller 独立管理。

---

## 快速开始

### 前置条件

- Controller 已部署且具有集群内读写 EdgionAcme、Secret、EdgionTls 的权限。
- Gateway 已部署；若使用 HTTP-01，需有对外提供 HTTP（如 80 端口）的监听器，且域名解析到该 Gateway。

### 最小示例（HTTP-01）

```yaml
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  name: example-acme
  namespace: default
spec:
  email: admin@example.com
  domains:
    - example.com
    - www.example.com
  challenge:
    type: http-01
    gatewayRef:
      name: my-gateway
      namespace: default
  storage:
    secretName: example-tls-cert
  autoEdgionTls:
    enabled: true
    parentRefs:
      - name: my-gateway
        namespace: default
```

创建后，Controller 会向 ACME 服务（默认 Let's Encrypt）注册账号、发起订单、完成 HTTP-01 校验并写入证书到 `storage.secretName` 指定的 Secret，并自动创建/更新 EdgionTls。

---

## 配置说明

### 基础字段

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `server` | String | 否 | Let's Encrypt 生产目录 | ACME 目录 URL，如 `https://acme-v02.api.letsencrypt.org/directory`；测试可用 staging 或 Pebble。 |
| `email` | String | **是** | - | ACME 账号联系邮箱。 |
| `domains` | Array\<String\> | **是** | - | 要申请证书的域名；含通配符（如 `*.example.com`）时须使用 DNS-01。 |
| `keyType` | String | 否 | `ecdsa-p256` | 证书密钥类型：`ecdsa-p256`、`ecdsa-p384`。 |

### 校验方式：challenge

`challenge` 是 tagged 枚举，由 `type` 区分两种形态，对应字段直接平铺在 `challenge` 下。

**HTTP-01** (`type: http-01`)：

| 字段 | 必填 | 说明 |
|------|------|------|
| `gatewayRef` | **是** | 用于提供校验响应的 Gateway 引用。 |

ACME 服务会访问 `http://<domain>/.well-known/acme-challenge/<token>`。Controller 将 token 写入 EdgionAcme 的 `status.activeChallenges` 并同步到 Gateway，Gateway 仅在存在未完成校验时响应该路径，校验结束后不再拦截。

**DNS-01** (`type: dns-01`)：

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `provider` | String | **是** | - | 支持：`cloudflare`、`alidns`；测试环境可用 `pebble`（配合 Pebble + challtestsrv）。 |
| `credentialRef` | object | **是** | - | 存 DNS API 凭据的 Secret 引用。 |
| `propagationTimeout` | int | 否 | 120 | 等待 DNS 传播的最长时间（秒）。 |
| `propagationCheckInterval` | int | 否 | 5 | 传播检查间隔（秒）。 |

Controller 根据 ACME 返回的 TXT 值，调用 DNS 提供方 API 创建 `_acme-challenge.<domain>` 的 TXT 记录，等待传播后再通知 ACME 校验；通过后删除 TXT。通配符域名必须使用 DNS-01。

**Cloudflare**：Secret 中需提供 `api-token`（具备 Zone.DNS:Edit 权限）。

**阿里云 DNS（alidns）**：Secret 中需提供 `access-key-id`、`access-key-secret`。

### 续期与重试：renewal

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `renewBeforeDays` | int | 30 | 在证书到期前多少天开始续期。 |
| `checkInterval` | int | 86400 | 定期检查续期需求的间隔（秒），例如 24 小时。 |
| `failBackoff` | int | 300 | 失败后首次重试的延迟基数（秒）；实际采用指数退避（300、1200、4800…），最多重试 5 次。 |

### 证书存储：storage

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `secretName` | String | **是** | 存放 `tls.crt`、`tls.key` 的 Secret 名称。 |
| `secretNamespace` | String | 否 | Secret 所在命名空间，缺省为 EdgionAcme 所在命名空间。 |

### 自动 EdgionTls：autoEdgionTls

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | Boolean | true | 是否自动创建/更新 EdgionTls。 |
| `name` | String | 否 | EdgionTls 名称，缺省为 `acme-<EdgionAcme 名称>`。 |
| `parentRefs` | []ParentRef | 否 | 自动创建的 EdgionTls 绑定的 Gateway。 |

---

## 手动触发

在需要立即重新发起申请或续期时（例如修正 DNS/网络后），可以手动触发一次处理，效果等同于“让 Controller 重新处理该资源”，并会重置失败重试计数。

### 方式一：修改资源（kubectl）

任意修改一次 EdgionAcme 的 metadata（例如加一条 annotation），Controller 会收到变更并重新处理该资源：

```bash
kubectl annotate edgionacme example-acme -n default edgion.io/trigger="$(date +%s)" --overwrite
```

### 方式二：Admin API

Controller 的 Admin API 提供专用触发接口：

```bash
curl -X POST "http://<controller-admin>:12101/api/v1/services/acme/<namespace>/<name>/trigger"
```

示例：触发 `default` 命名空间下 `example-acme` 的 ACME 流程：

```bash
curl -X POST "http://localhost:12101/api/v1/services/acme/default/example-acme/trigger"
```

返回示例：

```json
{
  "success": true,
  "data": "ACME check triggered for default/example-acme"
}
```

如果 ACME 服务未运行（例如当前未持有 leadership 的 Controller 副本），请求不会入队，响应会如实报告失败，而不是返回误导性的成功：

```json
{
  "success": false,
  "error": "ACME service not running; trigger for default/example-acme was not queued"
}
```

---

## 状态与排查

### 状态阶段（status.phase）

| 阶段 | 说明 |
|------|------|
| `Pending` | 尚未签发证书，等待首次签发或重试。 |
| `Issuing` | 正在执行 ACME 订单（校验或签发中）。 |
| `Ready` | 证书已签发且有效。 |
| `Renewing` | 正在续期。 |
| `Failed` | 最近一次操作失败；会按指数退避重试，最多 5 次。 |

### 常用状态字段

- `status.certificateSerial`、`status.certificateNotAfter`：当前证书序列号与过期时间（RFC 3339）。
- `status.lastFailureReason`、`status.lastFailureTime`：最近一次失败原因与时间。
- `status.secretName`、`status.edgionTlsName`：当前使用的 Secret 与 EdgionTls 名称。

### 常见问题

1. **HTTP-01 校验失败**  
   确认域名已解析到提供 HTTP 的 Gateway，且 80 端口可从公网访问；Controller 写入 token 后，Gateway 需能通过 List-Watch 收到并响应 `/.well-known/acme-challenge/<token>`。

2. **DNS-01 校验超时**  
   适当增大 `dns01.propagationTimeout`；确认 `credentialRef` 指向的 Secret 正确且 DNS 提供方 API 可用。

3. **证书一直处于 Failed**  
   查看 `status.lastFailureReason`；修复配置或网络后，使用上文“手动触发”重新发起一次，以重置重试计数。

---

## 示例

- **HTTP-01**：见仓库 `examples/test/conf/Services/acme/http01-example.yaml`。
- **DNS-01（Cloudflare）**：`examples/test/conf/Services/acme/dns01-cloudflare-example.yaml`。
- **DNS-01（阿里云）**：`examples/test/conf/Services/acme/dns01-alidns-example.yaml`。
- **本地集成测试（Pebble）**：`examples/test/conf/Services/acme/pebble/` 下的 Docker Compose 与说明。

与 EdgionTls 的配合使用见 [EdgionTls 用户指南](./edgion-tls.md)。

## 相关文档

- [TLS 终结](./tls-termination.md)
- [EdgionTls 扩展](./edgion-tls.md)
- [Gateway 资源总览](../overview.md)
- [HTTPS 监听器](../listeners/https.md)
