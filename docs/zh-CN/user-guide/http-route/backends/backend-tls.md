# 后端 TLS

使用标准 Gateway API 的 `BackendTLSPolicy` 资源配置 Gateway 到后端服务的 TLS 连接。

## 概述

默认情况下，Gateway 以明文连接后端。`BackendTLSPolicy` 告知 Gateway 将上游链路升级为 TLS，并根据 CA 验证后端证书，还可选择性地提供客户端证书以进行双向 TLS：

```
Client → [TLS] → Gateway → [TLS] → Backend
```

`BackendTLSPolicy` 是标准 Gateway API 资源（非 Edgion 扩展）。Edgion 内部将其作为 `gateway.networking.k8s.io/v1` 处理。对于只提供旧版 `v1alpha3` API 的集群，会通过回退机制接受并转换为同一内部模型；如果集群同时提供两个版本，建议优先使用 `v1`。

## TLS 后端支持

BoringSSL 是目前唯一受支持的 Gateway TLS 后端。保留的 rustls Cargo feature
仅作为未来开发入口，不提供构建、部署或 `BackendTLSPolicy` 行为保证。本页记录的
所有配置都应使用默认的 BoringSSL Gateway。

## 配置 BackendTLSPolicy

以下自定义 CA 示例要求使用 BoringSSL Gateway。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: backend-tls
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: backend-ca
    hostname: backend.internal
```

## 配置参考

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `targetRefs` | Array\<Object\> | **必填** | 此策略适用的 Service 列表 |
| `targetRefs[].group` | String | `""` | API 组；`""` 表示核心（Service） |
| `targetRefs[].kind` | String | `Service` | 目标类型，通常为 `Service` |
| `targetRefs[].name` | String | **必填** | Service 名称 |
| `targetRefs[].sectionName` | String | 无 | 可选的 Service 端口名称，用于将策略限定到单个端口 |
| `validation.hostname` | String | **必填** | 用于 TLS SNI 和证书验证的主机名 |
| `validation.caCertificateRefs` | Array\<Object\> | 无 | CA 引用（Secret 或 ConfigMap），仅 BoringSSL 支持；与 `wellKnownCACertificates` 互斥 |
| `validation.wellKnownCACertificates` | Enum | 无 | 设为 `System` 以信任系统 CA 包，与 `caCertificateRefs` 互斥 |
| `validation.subjectAltNames` | Array\<Object\> | 无 | 附加 SAN 允许列表；仅 BoringSSL 构建会执行。使用 BoringSSL 时，若设置，后端证书必须至少匹配其中一项 |
| `validation.subjectAltNames[].type` | Enum | **必填** | 允许值：`Hostname` \| `URI` |
| `validation.subjectAltNames[].hostname` | String | 无 | DNS 名称（`type: Hostname` 时），支持通配符（`*.example.com`） |
| `validation.subjectAltNames[].uri` | String | 无 | 完整 URI（`type: URI` 时），例如 SPIFFE ID |

> `caCertificateRefs` 和 `wellKnownCACertificates` 必须提供其中之一。同时设置两者会被拒绝（策略的 `Accepted` 状态变为 `Invalid`）。

### CA 引用来源

| `kind` | 读取的键 | 说明 |
|--------|----------|------|
| `Secret` | `ca.crt` | 包含 CA 包的标准 TLS Secret |
| `ConfigMap` | `ca.crt` | 包含 PEM CA 包的普通 ConfigMap |

CA 引用不存在或缺少 `ca.crt` 键时，`ResolvedRefs` 状态会被设为 `InvalidCACertificateRef`。

## CA 证书 Secret（仅 BoringSSL）

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: backend-ca
  namespace: production
type: kubernetes.io/tls
data:
  ca.crt: <base64-encoded-ca-cert>
```

## Subject Alternative Name 验证

> **TLS 后端能力限制**
>
> 附加 `subjectAltNames` 校验要求使用受支持的 BoringSSL Gateway 后端。

使用 BoringSSL 时，配置 `subjectAltNames` 后，Gateway 会在 TLS 握手期间验证后端证书的 SAN。不匹配则拒绝连接。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: backend-san-check
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: backend-ca
    hostname: backend.internal
    subjectAltNames:
      - type: Hostname
        hostname: backend.internal
      - type: URI
        uri: spiffe://cluster.local/ns/default/sa/backend
```

匹配规则：
- **Hostname**：精确匹配或通配符（`*.example.com` 匹配单级子域名）
- **URI**：精确匹配（例如 SPIFFE ID）
- 任一条目匹配即通过
- 不配置 `subjectAltNames` 时保持默认行为（仅使用 hostname 对 CA 进行验证）

## 对后端的双向 TLS（mTLS，仅 BoringSSL）

若要向后端提供客户端证书（mTLS），请使用 BoringSSL Gateway，并通过 Edgion 扩展选项 `edgion.io/client-certificate-ref` 引用客户端证书 Secret。该 Secret 必须与 `BackendTLSPolicy` 位于同一命名空间。

> **🔌 Edgion 扩展**
>
> `options.edgion.io/client-certificate-ref` 是 Edgion 对 `BackendTLSPolicy` 的特定扩展；面向后端 mTLS 的客户端证书并不属于标准 Gateway API。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: backend-mtls
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: secure-backend
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: backend-ca
    hostname: backend.internal
  options:
    # 与 BackendTLSPolicy 同命名空间的客户端证书 Secret，用于 gateway → backend mTLS
    edgion.io/client-certificate-ref: "backend-client-cert"
```

## 示例

### 示例 1：使用私有 CA 的内部服务 TLS（仅 BoringSSL）

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: internal-tls
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: internal-api
  validation:
    caCertificateRefs:
      - group: ""
        kind: Secret
        name: internal-ca
    hostname: internal-api.svc.cluster.local
```

### 示例 2：信任系统 CA 包

当后端持有公共受信 CA 颁发的证书时（例如通过 `ExternalName` Service 暴露的外部 HTTPS API），使用 `wellKnownCACertificates: System`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: BackendTLSPolicy
metadata:
  name: external-api-tls
  namespace: production
spec:
  targetRefs:
    - group: ""
      kind: Service
      name: external-api
  validation:
    hostname: api.example.com
    wellKnownCACertificates: System
```

## 状态与冲突

策略在关联的 Gateway（通过 `Service → Route → Gateway` 查找）上报告状态，而不是在策略本身上。观察 `Accepted` 和 `ResolvedRefs` 状态：

- `Accepted: Invalid` — 验证失败（例如同时设置了 `caCertificateRefs` 和 `wellKnownCACertificates`）。
- `Accepted: Conflicted` — 更高优先级的策略目标相同的 Service（以及 `sectionName`）。当多个策略目标同一 Service/端口时，最早的 `creationTimestamp` 获胜，并按 `{namespace}/{name}` 打破平局。不同的 `sectionName` 值不会产生冲突。
- `ResolvedRefs: InvalidCACertificateRef` — CA 引用不存在或缺少 `ca.crt` 键。

## 相关文档

- [Service 引用](./service-ref.md)
- [mTLS 配置](../../../ops-guide/infrastructure/mtls.md)
