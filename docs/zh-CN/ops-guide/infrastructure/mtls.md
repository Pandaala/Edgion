# mTLS 配置

> **🔌 Edgion 扩展**
> 
> mTLS 配置通过 `EdgionTls` CRD 实现，这是 Edgion 的扩展功能，不属于标准 Gateway API 规范。

配置双向 TLS（Mutual TLS）认证。

## 概述

mTLS 要求客户端和服务器双方都验证对方的证书：

```
Client [cert] ←→ [verify] Gateway [cert] ←→ [verify] Client
```

## 配置方式

使用 EdgionTls 资源配置 mTLS：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: mtls-config
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Require  # 要求客户端证书
    caCertificateRefs:
      - name: client-ca
```

## 配置参考

### clientAuth

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| mode | string | | None/Request/Require |
| caCertificateRefs | array | | 客户端 CA 证书 |

### clientAuth.mode

| 模式 | 说明 |
|------|------|
| None | 不验证客户端证书（默认） |
| Request | 请求客户端证书但不强制 |
| Require | 强制要求客户端证书 |

## CA 证书 Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
type: Opaque
data:
  ca.crt: <base64-encoded-ca-cert>
```

## 示例

### 示例 1: 强制 mTLS

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: strict-mtls
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Require
    caCertificateRefs:
      - name: trusted-client-ca
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: mtls-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        certificateRefs:
          - group: edgion.io
            kind: EdgionTls
            name: strict-mtls
```

### 示例 2: 可选 mTLS

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: optional-mtls
spec:
  secretRef:
    name: server-tls
  clientAuth:
    mode: Request  # 请求但不强制
    caCertificateRefs:
      - name: trusted-client-ca
```

## 客户端配置

使用 curl 测试 mTLS：

```bash
curl --cert client.crt --key client.key \
     --cacert server-ca.crt \
     https://example.com/api
```

---

## 控制面 mTLS（conf_sync gRPC）

除了数据面 mTLS（客户端 ↔ Gateway）外，Edgion 还支持 **控制面 gRPC 通道**（Controller ↔ Gateway 的 `conf_sync`）的 mTLS。这可以保护配置同步流量免受窃听和伪造。

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

在 `edgion-controller.toml` 中添加：

```toml
[conf_sync_security]
active = "primary"              # 选择当前生效的证书集

[[conf_sync_security.certs]]
name = "primary"
cert = "certs/grpc/primary/server.crt"
key  = "certs/grpc/primary/server.key"
ca   = "certs/grpc/primary/ca.crt"

# 可选：预置备用证书集，用于证书轮换
[[conf_sync_security.certs]]
name = "secondary"
cert = "certs/grpc/secondary/server.crt"
key  = "certs/grpc/secondary/server.key"
ca   = "certs/grpc/secondary/ca.crt"
```

证书路径相对于 `work_dir` 解析。

### Gateway 配置

在 `edgion-gateway.toml` 中添加：

```toml
[conf_sync_security]
active = "primary"

[[conf_sync_security.certs]]
name = "primary"
cert = "certs/grpc/primary/client.crt"
key  = "certs/grpc/primary/client.key"
ca   = "certs/grpc/primary/ca.crt"

# 可选：预置备用证书集，用于证书轮换
[[conf_sync_security.certs]]
name = "secondary"
cert = "certs/grpc/secondary/client.crt"
key  = "certs/grpc/secondary/client.key"
ca   = "certs/grpc/secondary/ca.crt"
```

启用 mTLS 后，Gateway 会自动将 `server_addr` 连接从 `http://` 升级为 `https://`。

> **向后兼容**：旧版单套证书格式仍受支持，无需迁移：
> ```toml
> [conf_sync_security.tls]
> cert = "certs/grpc/server.crt"
> key  = "certs/grpc/server.key"
> ca   = "certs/grpc/ca.crt"
> ```

### 配置参考

**`[conf_sync_security]`** — 顶层配置段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `active` | string | — | 当前生效的证书集名称。存在多个 `[[certs]]` 时必填。 |

**`[[conf_sync_security.certs]]`** — 证书集（表数组）

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | string | — | 证书集的唯一名称 |
| `cert` | string | — | PEM 证书路径（相对 work_dir） |
| `key` | string | — | PEM 私钥路径 |
| `ca` | string | — | CA 证书路径（用于验证对端） |
| `skip_tls` | bool | `false` | 紧急降级：跳过 TLS，以明文运行（输出 ERROR 日志） |

### 证书选择规则

| 配置状态 | 生效证书 |
|---------|---------|
| 单条 `[[certs]]` 且无 `active` | 自动选中唯一那条 |
| 多条 `[[certs]]` + `active = "name"` | 与 `name` 匹配的那条 |
| 多条 `[[certs]]` 且无 `active` | 启动报错 |
| 无 `[[certs]]`，存在旧格式 `[tls]` | 使用 `[tls]` 配置 |
| 无任何 mTLS 配置 | 明文 gRPC |

### 行为矩阵

| 配置状态 | 结果 |
|---------|------|
| 无 `[conf_sync_security]` 段 | 明文 gRPC（向后兼容） |
| active 证书有效 | mTLS gRPC |
| `active` 名称不匹配任何证书集 | 启动失败 |
| 证书文件缺失或无效 | Controller 启动失败 / Gateway 连接失败并重试 |
| `skip_tls = true` | 明文 + ERROR 级别警告日志 |

### 证书轮换

在保留回滚能力的前提下轮换证书：

1. 生成新证书集：`edgion-cli gen-certs --cert-name secondary ...`
2. 在 Controller 和 Gateway 的 TOML 中添加 `secondary` 条目，保持 `active = "primary"` 不变
3. 重启两个进程，验证新证书文件可正常加载
4. 切换：将两边配置改为 `active = "secondary"` 并重启
5. 若发现异常，改回 `active = "primary"` 并重启，即可立即回滚

### Kubernetes 部署

K8s 部署时，可将证书生成为 K8s Secret：

```bash
edgion-cli gen-certs --output k8s --cert-name primary
```

将 Secret 挂载到 Controller 和 Gateway Pod，并在 TOML 配置中引用挂载路径。

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

- [TLS 终结](../gateway/tls/tls-termination.md)
- [后端 TLS](../../user-guide/http-route/backends/backend-tls.md)
- [edgion-cli gen-certs](../edgion-cli.md)
