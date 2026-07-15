# mTLS 配置

> **🔌 Edgion 扩展**
>
> 下行（客户端→Gateway）的 mTLS 通过 `EdgionTls` CRD 配置，不属于标准 Gateway API 规范。

配置双向 TLS（mTLS），使 Gateway 在呈现自身服务端证书的同时也验证客户端证书。Edgion 在两个独立的地方支持 mTLS：**数据面**（客户端 → Gateway）通过 `EdgionTls` 配置，**控制面**（Controller ↔ Gateway gRPC）通过 `conf_sync_security` 配置。

## 概述

单向 TLS 中，客户端验证服务端。mTLS 添加了反向检查：服务端也根据受信任 CA 验证客户端证书。

```
one-way TLS:   client ──verify server cert──► Gateway
mTLS:          client ──verify server cert──► Gateway
               client ◄──verify client cert── Gateway
```

完整的 `EdgionTls` schema（TLS 版本控制、密码套件、OCSP、会话管理）详见 [EdgionTls](../gateway/tls/edgion-tls.md)。本页重点介绍与 mTLS 相关的字段。

## 配置

在 `EdgionTls` 资源中添加 `clientAuth` 块，并将其附加到 HTTPS 监听器，即可启用 mTLS。

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-config
  namespace: production
spec:
  hosts:
    - "api.example.com"
  secretRef:
    name: my-tls-secret              # 服务端证书
  clientAuth:
    mode: Mutual                     # 要求有效的客户端证书
    caSecretRef:
      name: client-ca                # 签发受信任客户端证书的 CA
```

### clientAuth 参数参考

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `mode` | Enum | `Terminate` | 客户端认证模式（见下文） |
| `caSecretRef` | Object | none | CA 证书 Secret。`mode` 为 `Mutual` 或 `OptionalMutual` 时**必填** |
| `verifyDepth` | Integer | `1` | 证书链验证深度（1-9） |
| `allowedSans` | Array\<String\> | none | Subject Alternative Name 允许列表 |
| `allowedCns` | Array\<String\> | none | Common Name 允许列表 |

#### `clientAuth.mode` 取值说明

| 模式 | 说明 |
|------|------|
| `Terminate` | 单向 TLS：只验证服务端证书（默认） |
| `Mutual` | 双向 TLS：客户端**必须**提供有效证书 |
| `OptionalMutual` | 可选双向 TLS：客户端证书可选，但提供时会被验证 |

### 将 EdgionTls 附加到监听器

在 HTTPS 监听器的 `certificateRefs` 中引用 `EdgionTls` 资源，并设置 `cert-provider` 选项，使 Gateway 使用扩展 TLS 栈：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        options:
          edgion.io/cert-provider: "edgion-tls"
        certificateRefs:
          - group: edgion.io
            kind: EdgionTls
            name: mtls-config
```

## CA 证书 Secret

CA Secret 持有签发受信任客户端证书的 CA 证书：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
  namespace: production
type: Opaque
data:
  ca.crt: <base64-encoded-ca-cert>
```

⚠️ 切勿将真实证书或密钥材料提交到源代码控制。上方的值均为占位符。

## 常见场景

### 1. 严格 mTLS（强制要求客户端证书）

每个客户端必须提供由受信任 CA 签发的证书，否则握手被拒绝：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: strict-mtls
  namespace: production
spec:
  hosts:
    - "api.example.com"
  secretRef:
    name: my-tls-secret
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: production
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        options:
          edgion.io/cert-provider: "edgion-tls"
        certificateRefs:
          - group: edgion.io
            kind: EdgionTls
            name: strict-mtls
```

### 2. 可选 mTLS（提供时验证）

允许已认证和匿名客户端同时访问，但验证任何提供的证书——适合分阶段推出时使用：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: optional-mtls
  namespace: production
spec:
  hosts:
    - "api.example.com"
  secretRef:
    name: my-tls-secret
  clientAuth:
    mode: OptionalMutual
    caSecretRef:
      name: client-ca
```

### 3. 限制允许的客户端身份

在 `Mutual` 模式上进一步通过允许的客户端 SAN 或 CN 进行限制：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: restricted-mtls
  namespace: production
spec:
  hosts:
    - "api.example.com"
  secretRef:
    name: my-tls-secret
  clientAuth:
    mode: Mutual
    caSecretRef:
      name: client-ca
    verifyDepth: 2
    allowedSans:
      - "client.example.com"
    allowedCns:
      - "Trusted Client"
```

## 使用 curl 测试

```bash
curl --cert client.crt --key client.key \
     --cacert server-ca.crt \
     https://api.example.com/api
```

`mode: Mutual` 时，没有有效客户端证书的客户端在 TLS 握手阶段被拒绝。

## 常见问题

### Q1：握手失败，提示"certificate required"。什么原因？

**A**：监听器处于 `Mutual` 模式，客户端未提供证书，或提供的证书不是由 `caSecretRef` 中 CA 签发的。请确认客户端证书链接到该 CA，且 `caSecretRef` 指向正确的 Secret。

### Q2：我的 `EdgionTls` 资源被忽略了——Gateway 使用了普通证书。为什么？

**A**：监听器必须通过 `tls.options."edgion.io/cert-provider": "edgion-tls"` 主动开启。没有该选项，监听器会回退到内置的 `certificateRefs` 处理，`EdgionTls` 的扩展字段（包括 `clientAuth`）不会生效。

### Q3：如何将已验证的客户端身份传递给后端？

**A**：在 `EdgionTls` 资源上设置注解 `edgion.io/expose-client-cert: "true"`。Gateway 随后会通过请求头将客户端证书信息转发给后端。

### Q4：证书 Secret 在其他命名空间。需要什么？

**A**：在 Secret 所在命名空间创建 `ReferenceGrant` 以授权引用。详见[跨命名空间引用](./reference-grant.md)。

---

## 控制面 mTLS（conf_sync gRPC）

除了数据面 mTLS（客户端 ↔ Gateway）外，Edgion 还支持 **Controller 与 Gateway 之间 gRPC 通道**（`conf_sync`）的 mTLS。这可以保护配置同步流量免受窃听和伪造。

### 生成证书

使用 `edgion-cli gen-certs` 生成证书束。每套证书通过名称标识（`--cert-name`，默认 `primary`），输出到 `{name}/` 子目录：

```bash
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name primary
```

这将在 `certs/grpc/primary/` 下生成：

| 文件 | 用途 |
|------|------|
| `ca.crt` | CA 证书（分发给双方） |
| `ca.key` | CA 私钥（仅签发用，不部署到运行时） |
| `server.crt` / `server.key` | Controller 服务端证书 |
| `client.crt` / `client.key` | Gateway 客户端证书 |

服务端证书默认包含 `localhost`、`127.0.0.1`、`::1` 作为 SAN。生产环境可添加自定义 SAN：

```bash
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name primary \
    --server-san edgion-controller.edgion-system.svc.cluster.local
```

预置备用证书集（用于轮换）：

```bash
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name secondary \
    --server-san edgion-controller.edgion-system.svc.cluster.local
```

### Controller 配置

在 `edgion-controller.yaml` 中添加：

```yaml
conf_sync_security:
  active: primary              # 选择当前生效的证书集
  certs:
    - name: primary
      cert: certs/grpc/primary/server.crt
      key: certs/grpc/primary/server.key
      ca: certs/grpc/primary/ca.crt
    # 可选：预置备用证书集，用于证书轮换
    - name: secondary
      cert: certs/grpc/secondary/server.crt
      key: certs/grpc/secondary/server.key
      ca: certs/grpc/secondary/ca.crt
```

证书路径相对于 `work_dir` 解析。

### Gateway 配置

在 `edgion-gateway.yaml` 中添加：

```yaml
conf_sync_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/primary/client.crt
      key: certs/grpc/primary/client.key
      ca: certs/grpc/primary/ca.crt
    # 可选：预置备用证书集，用于证书轮换
    - name: secondary
      cert: certs/grpc/secondary/client.crt
      key: certs/grpc/secondary/client.key
      ca: certs/grpc/secondary/ca.crt
```

启用 mTLS 后，Gateway 会自动将 `server_addr` 连接从 `http://` 升级为 `https://`。

> **向后兼容**：旧版单套证书格式仍受支持，无需迁移：
> ```yaml
> conf_sync_security:
>   tls:
>     cert: certs/grpc/server.crt
>     key: certs/grpc/server.key
>     ca: certs/grpc/ca.crt
> ```

### 配置参考

**`[conf_sync_security]`** — 顶层配置段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `active` | String | — | 当前生效的证书集名称。存在多个 `[[certs]]` 时必填。 |

**`[[conf_sync_security.certs]]`** — 证书集（表数组）

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | — | 证书集的唯一名称 |
| `cert` | String | — | PEM 证书路径（相对 work_dir） |
| `key` | String | — | PEM 私钥路径 |
| `ca` | String | — | CA 证书路径（用于验证对端） |
| `skip_tls` | Boolean | `false` | 紧急降级：跳过 TLS，以明文运行（输出 ERROR 日志） |

### 证书选择规则

| 配置状态 | 生效证书 |
|---------|---------|
| 单条 `[[certs]]` 且无 `active` | 自动选中唯一那条 |
| 多条 `[[certs]]` + `active = "name"` | 与 `name` 匹配的那条 |
| 多条 `[[certs]]` 且无 `active` | 启动报错 |
| 无 `[[certs]]`，存在旧格式 `[tls]` | 使用 `[tls]` 配置 |
| 无任何 mTLS 配置 | 启动失败（见下方行为矩阵） |

### 行为矩阵

| 配置状态 | 结果 |
|---------|------|
| 无 `conf_sync_security` 段 | **拒绝启动**（Controller 与 Gateway 均以非零码退出） |
| active 证书有效 | mTLS gRPC |
| `auto_ca: true`（Controller：只写它即可；Gateway：**还需**配 `tls` 证书路径） | mTLS gRPC（由 Controller 内置 CA 自动签发证书） |
| **Gateway** 上只写 `auto_ca: true`、没有证书路径 | 启动失败（auto_ca 意味着强制 mTLS，客户端证书必须能从磁盘读到） |
| `active` 名称不匹配任何证书集 | 启动失败 |
| 证书文件缺失或无效 | Controller 启动失败 / Gateway 连接失败并重试 |
| `skip_tls: true` | 明文 + ERROR 级别警告日志 |

### 迁移说明：`conf_sync_security` 现为必填

conf_sync 通道会把**解析后的 Secret**（TLS 私钥、后端凭证）从 Controller 推送到 Gateway。
因此该通道上的明文必须是一个显式的、可审计的选择，绝不能是"忘记写配置块"的静默后果。

过去省略 `conf_sync_security` 意味着"明文 gRPC"，现在则会让**两个二进制都拒绝启动**（非零退出码）。
如果你的部署依赖于"省略该段"，请在升级前从以下三者中选择其一：

- **Kubernetes（推荐）**：启用 Controller 内置 CA。**两端配置并不对称** —— 把 Controller 的配置块直接
  照抄到 Gateway 会导致 Gateway 以非零码退出：

  ```yaml
  # Controller —— 只写 auto_ca 即可（它在内存中自签服务端身份）
  conf_sync_security:
    auto_ca: true

  # Gateway —— auto_ca 加上 Controller 自动签发的客户端证书路径
  # （Secret `edgion-conf-sync-certs`，由官方 manifest 挂载）
  conf_sync_security:
    auto_ca: true
    tls:
      cert: /etc/edgion/conf-sync-certs/client.crt
      key: /etc/edgion/conf-sync-certs/client.key
      ca: /etc/edgion/conf-sync-certs/ca.crt
  ```

- **文件模式 mTLS**：配置 `conf_sync_security.certs`（或旧格式 `conf_sync_security.tls`），提供 `cert` / `key` / `ca` 路径。
- **继续使用明文**：显式声明。

```yaml
conf_sync_security:
  tls:
    skip_tls: true    # 不安全：conf_sync 走明文。仅限本地开发 / 非生产环境。
```

明文（`skip_tls: true`）在两端启动时仍会打印 ERROR 日志，并置位
`edgion_system_error_state{reason="skip_tls"}`，使这一降级状态在监控中保持可见。

### 证书轮换

在保留回滚能力的前提下轮换证书：

1. 生成新证书集：`edgion-cli gen-certs --cert-name secondary ...`
2. 在 Controller 和 Gateway 的 YAML 中添加 `secondary` 条目，保持 `active: primary` 不变
3. 重启两个进程，验证新证书文件可正常加载
4. 切换：将两边配置改为 `active: secondary` 并重启
5. 若发现异常，改回 `active: primary` 并重启，即可立即回滚

### Kubernetes 部署

K8s 部署时，可将证书生成为 K8s Secret：

```bash
edgion-cli gen-certs --output k8s --cert-name primary
```

将 Secret 挂载到 Controller 和 Gateway Pod，并在 YAML 配置中引用挂载路径。

### 验证

启用 mTLS 后，通过检查日志确认：

```
# Controller 日志
conf_sync mTLS enabled for gRPC server

# Gateway 日志
conf_sync mTLS enabled for gRPC client
```

也可以尝试明文 gRPC 连接 Controller 的 gRPC 端口——应被拒绝。

## 相关文档

- [EdgionTls](../gateway/tls/edgion-tls.md)
- [TLS 终结](../gateway/tls/tls-termination.md)
- [后端 TLS](../../user-guide/http-route/backends/backend-tls.md)
- [edgion-cli gen-certs](../edgion-cli.md)
