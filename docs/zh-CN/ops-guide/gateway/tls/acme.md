# ACME 自动证书（EdgionAcme）

> **🔌 Edgion 扩展**
>
> `EdgionAcme` 是 Edgion 自定义 CRD，用于通过 ACME 协议（如 Let's Encrypt）自动申请与续期 TLS 证书，并自动创建/更新 EdgionTls 与 Secret。

## 什么是 EdgionAcme？

EdgionAcme 在 Controller 侧运行 ACME 客户端，完成域名校验、证书签发与续期，并将证书写入 Kubernetes Secret、可选自动创建/更新 EdgionTls 资源，供 Gateway 使用。

**主要能力**：

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
    http01:
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
| `server` | string | 否 | Let's Encrypt 生产目录 | ACME 目录 URL，如 `https://acme-v02.api.letsencrypt.org/directory`；测试可用 staging 或 Pebble。 |
| `email` | string | **是** | - | ACME 账号联系邮箱。 |
| `domains` | []string | **是** | - | 要申请证书的域名；含通配符（如 `*.example.com`）时须使用 DNS-01。 |
| `keyType` | string | 否 | `ecdsa-p256` | 证书密钥类型：`ecdsa-p256`、`ecdsa-p384`。 |

### 校验方式：challenge

| 字段 | 说明 |
|------|------|
| `challenge.type` | `http-01` 或 `dns-01`。 |
| `challenge.http01` | HTTP-01 时必填，需指定 `gatewayRef`（用于提供校验响应的 Gateway）。 |
| `challenge.dns01` | DNS-01 时必填，需指定 DNS 提供商与凭据。 |

**HTTP-01**：ACME 服务会访问 `http://<domain>/.well-known/acme-challenge/<token>`。Controller 将 token 写入 EdgionAcme 并同步到 Gateway，Gateway 仅在存在未完成校验时响应该路径，校验结束后不再拦截。

**DNS-01**：Controller 根据 ACME 返回的 TXT 值，调用 DNS 提供方 API 创建 `_acme-challenge.<domain>` 的 TXT 记录，等待传播后再通知 ACME 校验；通过后删除 TXT。通配符域名必须使用 DNS-01。

### DNS-01 配置（dns01）

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `provider` | string | **是** | - | 支持：`cloudflare`、`alidns`；测试环境可用 `pebble`（配合 Pebble + challtestsrv）。 |
| `credentialRef` | object | **是** | - | 存 DNS API 凭据的 Secret 引用。 |
| `propagationTimeout` | int | 否 | 120 | 等待 DNS 传播的最长时间（秒）。 |
| `propagationCheckInterval` | int | 否 | 5 | 传播检查间隔（秒）。 |

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
| `secretName` | string | **是** | 存放 `tls.crt`、`tls.key` 的 Secret 名称。 |
| `secretNamespace` | string | 否 | Secret 所在命名空间，缺省为 EdgionAcme 所在命名空间。 |

### 自动 EdgionTls：autoEdgionTls

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | bool | true | 是否自动创建/更新 EdgionTls。 |
| `name` | string | 否 | EdgionTls 名称，缺省为 `acme-<EdgionAcme 名称>`。 |
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
curl -X POST "http://<controller-admin>:8080/api/v1/services/acme/<namespace>/<name>/trigger"
```

示例：触发 `default` 命名空间下 `example-acme` 的 ACME 流程：

```bash
curl -X POST "http://localhost:8080/api/v1/services/acme/default/example-acme/trigger"
```

返回示例：

```json
{
  "success": true,
  "data": "ACME check triggered for default/example-acme"
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
