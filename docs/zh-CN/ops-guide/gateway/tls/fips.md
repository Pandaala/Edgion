# FIPS 模式（BoringSSL）

> **🔒 合规特性**
>
> FIPS 模式让 Gateway 数据面运行在经过 FIPS 验证的 BoringSSL 加密模块之上，并使下行（客户端 →
> Gateway）TLS 默认采用 FIPS 批准的密码套件与椭圆曲线集合。它是**构建 / 镜像层面的能力**，而非运行时
> 开关：验证模块在使用 `fips` cargo feature 构建 Gateway 镜像时编译进去。没有针对单个 listener 或单条
> 连接的 FIPS 开关。

FIPS 模式面向必须以 FIPS 合规姿态运行的部署（美国联邦、受监管行业）。当 Gateway 二进制是 FIPS 构建
时，有两点成立：

1. **验证模块**——链接的加密实现是经过 FIPS 验证的 BoringSSL 模块（`boring::fips::enabled()` 为
   true）。
2. **默认即批准套件**——下行服务端 TLS 默认采用 FIPS 批准的 TLS 1.2 密码套件与 ECDH 曲线，除非运维在
   listener 上显式覆盖密码套件。

本特性仅适用于 **BoringSSL** 后端（默认 TLS 后端）。`rustls` 后端不支持 FIPS。

## FIPS 构建强制哪些内容

在 FIPS 构建上，下行 TLS 终结 listener 在未被覆盖时应用批准的默认值：

| 方面 | FIPS 批准默认值 |
|------|-----------------|
| TLS 1.2 密码套件 | `ECDHE-ECDSA-AES128-GCM-SHA256`、`ECDHE-RSA-AES128-GCM-SHA256`、`ECDHE-ECDSA-AES256-GCM-SHA384`、`ECDHE-RSA-AES256-GCM-SHA384`（排除 ChaCha20、CBC-SHA1、RC4、3DES） |
| ECDH 曲线 | `P-256`、`P-384`、`P-521` |
| 最低 TLS 版本 | 不变——BoringSSL 本就默认 TLS 1.2+，符合 FIPS 要求 |
| TLS 1.3 套件 | 不变（见[已知限制](#已知限制)） |

运维覆盖始终优先：若某 listener 的 `EdgionTls.spec.ciphers` 已设置，则使用该列表而非 FIPS 默认值
（批准的曲线仍然生效）。参见[按 listener 覆盖](#按-listener-覆盖密码套件)。

## 启用 FIPS

FIPS 在**镜像构建期**启用，而非运行时。发布 / CI 的 Docker 构建以 `fips` cargo feature 编译 Gateway
并将预编译的验证模块烘焙进镜像。默认的 workspace 构建和本地 `cargo build` **不**包含 FIPS——这样本地 /
开发构建保持精简且始终可编译。

没有任何配置字段或环境变量能把 FIPS “打开”——你需要部署一个 FIPS 镜像。下面的配置开关控制的是**断言与
策略**，而不是验证模块是否存在。

## 配置

有两个 gateway-local（进程级、非集群同步）的启动开关控制 FIPS 行为。两者都位于 Gateway YAML 配置中，
并有对应的环境变量。完整说明见 [Gateway 配置](../../configuration/gateway-config.md)。

### `require_fips`——fail-close 启动守卫

在启动时断言运行的二进制是 FIPS 构建。在**必须**为 FIPS 的节点上使用它，以便在开始服务流量前捕获误配的
（非 FIPS）镜像。

| 字段 | 类型 | 默认 | 环境变量 | 说明 |
|------|------|------|----------|------|
| `require_fips` | `bool` | `false` | `EDGION_REQUIRE_FIPS` | 启动时要求 FIPS 验证加密模块存在 |

- `require_fips: false`（默认）——无论二进制是否为 FIPS 构建都正常启动。
- `require_fips: true` 且为 **FIPS 构建**——守卫通过；批准默认值自动生效。
- `require_fips: true` 且为**非 FIPS 构建**——Gateway 记录明确错误并在启动时**以非零码退出**
  （fail-close），不会静默降级。

它被刻意设计为 gateway-local 字段而非集群同步字段：坏值无法被热推送而拖垮整个集群，且会在首次运行时暴
露。

```yaml
# edgion-gateway.yaml
require_fips: true
```

```bash
# 或通过环境变量（truthy：1 / true / yes / on）
EDGION_REQUIRE_FIPS=1 ./edgion-gateway --config-file config/edgion-gateway.yaml
```

### `fips_policy`——在非 FIPS 构建上强制启用批准策略

即使二进制不是 FIPS 构建，也在下行 listener 上强制启用 FIPS 批准的密码套件 / 曲线**策略**。这只是策略
开关——它**不**提供验证模块。

| 字段 | 类型 | 默认 | 环境变量 | 说明 |
|------|------|------|----------|------|
| `fips_policy` | `bool` | `false` | `EDGION_FIPS_POLICY` | 无论验证模块是否存在都强制批准的密码套件 / 曲线 |

- `fips_policy: false`（默认）——强制策略跟随构建：真正的 FIPS 构建自动强制批准集合；非 FIPS 构建不
  强制。
- `fips_policy: true`——在普通 BoringSSL 构建上也强制批准的密码套件 / 曲线集合。

强制门实际上是 `fips_module_available() OR fips_policy`。两个用途：

1. 在没有 FIPS 工具链或模块的情况下于集成测试中验证 FIPS 密码套件 / 曲线行为（批准套件限制是纯 SSL 配
   置，在任何 BoringSSL 构建上都有效）。
2. 作为纵深防御主动开启批准套件限制。

> ⚠️ `fips_policy: true` **不会**让非 FIPS 二进制变得 FIPS 合规。真正的 FIPS 合规要求二进制是 FIPS
> 构建（验证模块）。此开关仅镜像**密码套件 / 曲线策略**。

## 按 listener 覆盖密码套件

FIPS 默认值可通过既有的 `EdgionTls.spec.ciphers` 字段按 listener 覆盖——无需新增 schema。必须偏离的
listener（例如为了与需要特定套件的客户端互通）显式设置 `ciphers`，该列表将优先于 FIPS 默认值：

```yaml
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: custom-ciphers
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: https
  hosts:
    - secure.example.com
  secretRef:
    name: secure-example-com-tls
    namespace: production
  minTlsVersion: TLS1_2
  ciphers:
    - ECDHE-ECDSA-AES256-GCM-SHA384
    - ECDHE-RSA-AES256-GCM-SHA384
```

FIPS 构建上的决策顺序：

```text
运维设置了 spec.ciphers        -> 使用它们（覆盖优先）
否则 FIPS 构建 / fips_policy    -> 应用批准的默认密码套件 + 曲线
否则                            -> 保留 BoringSSL 默认（非 FIPS 行为）
```

即使 `ciphers` 被覆盖，批准的 ECDH 曲线仍然生效。密码套件字段的完整语义见 [EdgionTls](./edgion-tls.md)。

## 验证 FIPS 是否生效

Gateway 通过可观测性而非业务 API 暴露 FIPS 状态：

- **启动日志**——一条 INFO 记录报告模块状态、构建期注入的模块标识符，以及批准默认值是否生效。
- **指标**——`edgion_gateway_fips_module_enabled`（gauge，`0`/`1`）反映 Layer-1 事实：链接了验证模块
  时为 `1`。通过 Gateway 指标端点抓取（见 [监控指标](../../observability/metrics.md)）。

从客户端确认协商到的套件：

```bash
# 应协商到批准的 TLS 1.2 套件；非批准套件（如 ChaCha20）被拒绝
openssl s_client -connect secure.example.com:443 -tls1_2 \
  -cipher 'ECDHE-RSA-CHACHA20-POLY1305' </dev/null
# -> FIPS 构建上握手失败（ChaCha20 不在批准的 TLS 1.2 集合中）

openssl s_client -connect secure.example.com:443 -tls1_2 \
  -cipher 'ECDHE-RSA-AES256-GCM-SHA384' </dev/null
# -> 成功
```

## 已知限制

FIPS 模式覆盖了常见的下行姿态，但存在若干有记录的缺口（完整设计见
`docs/superpowers/specs/2026-07-02-boringssl-fips-design.md`）：

- **BoringSSL 无法限制 TLS 1.3 密码套件。** `TLS_CHACHA20_POLY1305_SHA256` 在 TLS 1.3 上仍可协商。
  批准套件强制仅在 **TLS 1.2** 生效。若要彻底排除 TLS 1.3 ChaCha20，必须禁用 TLS 1.3（默认不禁用）。
- **上行（Gateway → 后端）TLS 密码套件不受 FIPS 限制。** Pingora 0.8.1 连接器使用静态密码套件列表，无
  per-peer 钩子。上行依赖验证模块加既有的服务端证书校验。
- **不强制密钥类型策略**（例如拒绝 Ed25519 叶子密钥）。
- **TLS 1.3 会话恢复（PSK）** 跳过 per-connection 的密码套件 / 曲线限制，因此恢复的 TLS 1.3 握手的临时
  群组不会被重新限制。这与既有的 per-connection `EdgionTls.ciphers` 机制一致，且属于上述 TLS 1.3 限制
  范围内。

## 故障排查

| 现象 | 可能原因 | 处理 |
|------|----------|------|
| Gateway 启动时报 FIPS 错误并非零退出 | 非 FIPS 镜像上设了 `require_fips: true` | 部署 FIPS 构建镜像；若该节点不要求 FIPS，则去掉 `require_fips` |
| `edgion_gateway_fips_module_enabled` 为 `0` | 二进制不是 FIPS 构建 | 运行的镜像未以 `fips` feature 编译——改用 FIPS 镜像 |
| ChaCha20 客户端在 TLS 1.2 上失败，但你以为能用 | FIPS 批准默认套件排除了 ChaCha20 | 在该 listener 显式设置 `EdgionTls.spec.ciphers`，或将这些客户端路由到非 FIPS listener |
| ChaCha20 在 TLS 1.3 上仍能协商 | BoringSSL 无法限制 TLS 1.3 套件（有记录的限制） | 固定 `minTlsVersion: TLS1_2` 并在该 listener 禁用 TLS 1.3（若必须排除） |
| 本地（非 FIPS）构建上未应用批准策略 | 未设置 `fips_policy` | 设置 `fips_policy: true` 或 `EDGION_FIPS_POLICY=1` 以在无 FIPS 模块时验证策略 |

## 相关文档

- [EdgionTls](./edgion-tls.md)——密码套件 / 最低 TLS 版本控制与 `ciphers` 覆盖
- [TLS 终结](./tls-termination.md)
- [Gateway 配置](../../configuration/gateway-config.md)——`require_fips` / `fips_policy` 参考
- [监控指标](../../observability/metrics.md)——抓取 `edgion_gateway_fips_module_enabled`
