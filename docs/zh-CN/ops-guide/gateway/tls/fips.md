# FIPS 模式（BoringSSL）

> **🔒 FIPS 模式构建特性**
>
> FIPS 模式让 Gateway 数据面运行在 BoringSSL 的 FIPS 模式加密模块之上，并使下行（客户端 →
> Gateway）TLS 默认采用 FIPS 批准的密码套件与椭圆曲线集合。它是**构建 / 镜像层面的能力**，而非运行时
> 开关：模块在使用 `fips` cargo feature 构建 Gateway 镜像时编译进去。没有针对单个 listener 或单条
> 连接的 FIPS 开关。

FIPS 模式面向需要 FIPS 对齐加密姿态的部署。当 Gateway 二进制是 FIPS 构建时，有两点成立：

1. **FIPS 模式模块**——BoringSSL 报告 FIPS 模式（`boring::fips::enabled()` 为 `true`），且运行时
   完整性自检通过。
2. **默认即批准套件**——下行服务端 TLS 默认采用 FIPS 批准的 TLS 1.2 密码套件与 ECDH 曲线，除非运维在
   listener 上显式覆盖密码套件。

本特性仅适用于 **BoringSSL** 后端（默认 TLS 后端）。`rustls` 后端不支持 FIPS。

> `boring::fips::enabled()`、模块指标和完整性自检只能证明运行模式与已构建行为，不能识别 CMVP 验证
> 模块流、证书范围、制品来源，也不能证明整个部署合规。需要提出合规声明的运维方必须按照适用计划和平台
> 另行核验这些事项。

## FIPS 构建强制哪些内容

在 FIPS 构建上，下行 TLS 终结 listener 在未被覆盖时应用批准的默认值：

| 方面 | FIPS 批准默认值 |
|------|-----------------|
| TLS 1.2 密码套件 | `ECDHE-ECDSA-AES128-GCM-SHA256`、`ECDHE-RSA-AES128-GCM-SHA256`、`ECDHE-ECDSA-AES256-GCM-SHA384`、`ECDHE-RSA-AES256-GCM-SHA384`（排除 ChaCha20、CBC-SHA1、RC4、3DES） |
| ECDH 曲线 | `P-256`、`P-384`、`P-521` |
| 最低 TLS 版本 | 不变——BoringSSL 本就默认 TLS 1.2+，符合 FIPS 要求 |
| TLS 1.3 套件 | 不变（见[已知限制](#已知限制)） |

密码套件优先级为 EdgionTls 显式列表、EGC `spec.downstreamTls.ciphers`、
FIPS 批准默认值。任一层的显式 `[]` 使用后端默认套件。最低版本同样由 EdgionTls
覆盖 EGC 默认值。曲线仅由 EGC `spec.downstreamTls.ecdhCurves` 设置：缺省采用
NIST 曲线，显式 NIST 子集保留顺序，包含 X25519 则拒绝握手。密码套件覆盖不会
放宽曲线限制；空列表、重复或不支持的曲线均无效。配置更新仅影响新握手，现有连接
保留协商结果。参见[下游 TLS 默认策略](../../configuration/edgion-gateway-config.md)。

## 启用 FIPS

FIPS 在**镜像构建期**启用，而非运行时。普通发布 workflow、默认 workspace 构建和本地
`cargo build` **不**包含 FIPS。请通过仓库提供的 Linux 多架构 builder 显式构建：

```bash
FEATURES="allocator-jemalloc,fips" \
  ./cicd/build-image.sh --arch amd64,arm64 --compile-only
```

规范的 FIPS 源码构建支持 `amd64` 与 `arm64`。Boring 5.4 随附的源码已包含上游 arm64 完整性
哈希重定位修复。普通与 FIPS 构建共用对应架构的 builder，直接使用 crate 随附源码，不再单独检出
源码或下载补丁。普通构建和发布 workflow 仍不启用 FIPS。去掉 `--compile-only` 即会继续打包镜像。
可设置 `CARGO_BUILD_JOBS` 限制构建容器中的编译并发数。`BORING_BSSL_FIPS_PATH` 仍是上游提供的
预构建目录入口，不属于 Edgion 规范脚本的支持契约。

选择 `fips` 后，脚本会对每个 release 二进制自动执行隔离验证。Gateway 必须能以
`EDGION_REQUIRE_FIPS=1` 启动，报告 `edgion_gateway_fips_module_enabled 1`，拒绝仅提供 ChaCha20 的
TLS 1.2 客户端，并接受批准的 AES-GCM 客户端，构建才会成功。通用集成套件继续使用普通 BoringSSL
构建，因为其中的弱密码兼容性用例有意验证另一种安全姿态。

没有任何配置字段或环境变量能把 FIPS “打开”——你需要部署一个显式构建并通过验证的 FIPS 镜像。下面的
配置开关控制的是**断言与策略**，而不是 BoringSSL FIPS 模式是否存在。

## 主机 FIPS 检测与 `auto` 默认值

Gateway 还会在启动时通过读取一次 `/proc/sys/crypto/fips_enabled` 来检测**宿主机内核**是否处于 FIPS
模式。BoringSSL 静态链接进二进制文件，因此宿主机的 FIPS 模式对进程内实际执行的加密代码没有任何影响——
这项检测不能把一个非 FIPS 二进制变成 FIPS 模式二进制。它驱动两件互相独立的事情：

1. **姿态护栏**——如果宿主机内核报告处于 FIPS 模式，但本二进制未运行 BoringSSL FIPS 模式，Gateway 会在启动时发出
   一条 WARN，指出被限制的套件与逃生舱口，然后正常启动（fail-fast 仍然是通过 `require_fips` 的可选行
   为）。
2. **`auto` 策略默认值**——`fips_policy`（见下文）默认为 `auto`，只要宿主机是 FIPS 节点，即使二进制不
   是 FIPS 构建，也会强制启用批准的密码套件 / 曲线策略。

**对运维的影响**：在一个 FIPS 节点上，**完全没有任何 FIPS 相关配置**的 Gateway，现在会自动把 TLS 1.2
限制为四个批准的 AES-GCM 密码套件与 P-256/P-384/P-521 曲线。只能协商 ChaCha20 或 CBC 套件的 TLS 1.2
客户端将握手失败。这是一个纯粹由节点内核标志驱动的真实行为变化——Gateway 侧配置并未改变。逃生舱口：

- `fips_policy: off`——为该部署禁用强制策略（真正的 FIPS 构建仍会无条件强制批准的密码套件配置；`off`
  只能禁用 module-less/auto 这一路径）。
- `EdgionTls.spec.ciphers`——为该工作负载 / listener 显式设置密码套件列表；运维覆盖始终优先于任何 FIPS
  默认值。

### `Unknown` 主机状态

读取该标志文件最终会落在三种状态之一，而不只是 enabled/disabled：

| 状态 | 何时出现 | 影响 |
|------|----------|------|
| `Enabled` | 文件可读，内容为 `1` | `auto` 生效强制；如果本二进制非 FIPS，mismatch 护栏可能触发 |
| `Disabled` | 文件可读但内容非 `1`，或文件不存在（ENOENT / 非 Linux） | `auto` 不强制 |
| `Unknown` | 文件存在但无法读取（例如被加固 / 屏蔽的 `/proc`） | 记录一条 **WARN**，将 `edgion_gateway_fips_host_detect_error` 指标置为 `1`，**不**触发姿态护栏，也**不**在 `auto` 下收紧密码套件 |

`Unknown` 在流量上选择失败开放（无法确定的主机状态不能作为违规证据），但会在日志与指标中刻意保持醒目，
因为屏蔽 `/proc` 的加固节点恰恰是本特性面向的高保障环境。

### 已知限制：沙箱化容器运行时

在基于 VM 或沙箱隔离的容器运行时下——Kata Containers、Firecracker microVM、gVisor——
`/proc/sys/crypto/fips_enabled` 反映的是**客户机 / 沙箱内核**，而非物理节点。读取会*成功*，但返回一个
有效却错误的值，因此没有任何检测状态或日志能捕捉到这种情况。在默认的命名空间共享运行时（runc/containerd）
下，该文件能正确反映宿主机内核。

**在沙箱化运行时上，运维必须显式设置 `fips_policy`**（`on` 或 `off`），而不要依赖 `auto`。

## 配置

有两个 gateway-local（进程级、非集群同步）的启动开关控制 FIPS 行为。两者都位于 Gateway YAML 配置中，
并有对应的环境变量。完整说明见 [Gateway 配置](../../configuration/gateway-config.md)。

### `require_fips`——fail-close 启动守卫

在启动时断言运行的二进制是 FIPS 构建。在**必须**为 FIPS 的节点上使用它，以便在开始服务流量前捕获误配的
（非 FIPS）镜像。

| 字段 | 类型 | 默认 | 环境变量 | 说明 |
|------|------|------|----------|------|
| `require_fips` | `bool` | `false` | `EDGION_REQUIRE_FIPS` | 启动时要求 BoringSSL FIPS 模式存在 |

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

### `fips_policy`——无论 FIPS 模式是否存在都选择批准策略

一个三态开关，用于在下行 listener 上选择 FIPS 批准的密码套件 / 曲线**策略**，与二进制是否为 FIPS 构建
无关。这只是策略开关——它**不**提供 BoringSSL FIPS 模式。

| 字段 | 类型 | 默认 | 环境变量 | 说明 |
|------|------|------|----------|------|
| `fips_policy` | `"auto" \| "on" \| "off"` | `auto` | `EDGION_FIPS_POLICY` | 无论 FIPS 模式是否存在都选择批准的密码套件 / 曲线强制策略 |

- `auto`（默认）——跟随宿主机：当[主机 FIPS 检测](#主机-fips-检测与-auto-默认值)报告宿主机内核处于
  FIPS 模式时强制启用批准集合，否则不强制。这只是密码套件层面的策略对齐，不是合规声明。
- `on`——即使在普通 BoringSSL 构建上，也无条件强制启用批准的密码套件 / 曲线集合。
- `off`——永不强制启用该策略。真正的 FIPS 构建仍会无条件强制批准配置；`off` 只能禁用 module-less
  （强制）路径。

强制门实际上是 `fips_module_available() OR fips_policy_forced`，其中 `fips_policy_forced` 由该设置
（以及 `auto` 时的主机状态）解析得出。`on` 的两个用途：

1. 在没有 FIPS 工具链或模块的情况下于集成测试中验证 FIPS 密码套件 / 曲线行为（批准套件限制是纯 SSL 配
   置，在任何 BoringSSL 构建上都有效）。
2. 作为纵深防御主动开启批准套件限制。

> ⚠️ `fips_policy: on`（或 `auto` 默认值在 FIPS 主机上收紧）**不会**让非 FIPS 二进制变得 FIPS 合规。
> FIPS 模式二进制是 Edgion 这一姿态的必要条件，但本身不能证明 CMVP 证书范围、制品来源、平台验证或部署
> 合规。此字段仅镜像**密码套件 / 曲线策略**。

**取值与优先级。** 只接受上面这三个字符串——没有历史遗留的布尔值，也没有历史遗留的 truthy/falsey 令
牌。YAML 布尔值（`fips_policy: true`）会触发反序列化硬错误；而 YAML 1.1 会把裸写的 `on` / `off` 标量
解析为布尔值，因此配置文件中这两个取值必须加引号：

```yaml
# edgion-gateway.yaml
fips_policy: "off"   # 或 "on"；auto 无需加引号
```

解析优先级，从高到低：**环境变量 → CLI → 配置文件 → `auto`**。

```bash
# 环境变量只接受 auto/on/off；其他任何取值（包括旧的 1/true/0/false）都是无法识别的
# 令牌，会被 WARN 并忽略，回退到 CLI/配置文件，而不是被当成 off
EDGION_FIPS_POLICY=off ./edgion-gateway --config-file config/edgion-gateway.yaml

# CLI：裸标志表示 on；--fips-policy=auto / --fips-policy=off 同样有效
./edgion-gateway --fips-policy --config-file config/edgion-gateway.yaml
```

`--fips-policy` 与 `--require-fips` 现在**真正生效**：此前两者都会被 clap 解析，随后又被配置合并逻辑
静默丢弃，只有 YAML 和环境变量能真正到达运行中的 Gateway。这个合并缺陷已修复——传入该 CLI 标志时其值
会被采纳，省略该标志则保留配置文件中的原值。

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

决策顺序：

```text
设置了 EdgionTls.spec.ciphers       -> 使用该列表；[] 使用后端默认套件
否则设置了 EGC downstreamTls.ciphers -> 使用该列表；[] 使用后端默认套件
否则 fips_policy_active() 为真      -> 应用批准的默认密码套件
  （FIPS 构建上恒真，或 fips_policy=on，或 fips_policy=auto 且宿主机为 FIPS 节点）
否则                                -> 保留 BoringSSL 默认（非 FIPS 行为）
```

即使 `ciphers` 被覆盖，批准的 ECDH 曲线仍然生效。密码套件字段的完整语义见 [EdgionTls](./edgion-tls.md)。

## 验证 FIPS 是否生效

Gateway 通过可观测性而非业务 API 暴露 FIPS 状态：

- **启动日志**——一条 INFO 记录（`"FIPS status resolved"`）报告模块状态、检测到的主机 FIPS 状态、解析
  后的 `fips_policy` 模式，以及批准配置是否生效。
- **指标**——四个 gauge，全部在启动时无条件设置（从不采用惰性注册，因此一个缺失的序列永远不会掩盖
  `== 0` 这一条件）：

  | 指标 | 含义 |
  |------|------|
  | `edgion_gateway_fips_module_enabled` | BoringSSL 在本二进制中报告 FIPS 模式时为 `1`；该值不能证明 CMVP 证书范围或构建来源。 |
  | `edgion_gateway_fips_host_enabled` | 宿主机内核报告处于 FIPS 模式（`/proc/sys/crypto/fips_enabled`）时为 `1` |
  | `edgion_gateway_fips_host_detect_error` | 主机检测无法读取标志文件（`HostFipsState::Unknown`）时为 `1`——此时护栏与 `auto` 默认值均处于失效状态 |
  | `edgion_gateway_fips_policy_active{module_enabled="true\|false"}` | 该进程上批准的密码套件 / 曲线配置实际生效时为 `1`；标签记录 BoringSSL FIPS 模式信号 |

  通过 Gateway 指标端点抓取（见 [监控指标](../../observability/metrics.md)）。

> ⚠️ **`fips_policy_active{module_enabled="false"}` 不是合规通过的标志。** 它意味着批准的密码套件运行时
> 没有 BoringSSL FIPS 模式——这正是 `auto`/`on` 在非 FIPS 二进制上产生的结果。只检查协商套件的仪表盘
> 面板或外部密码套件扫描器，会看到与真正 FIPS 构建相同的批准套件，从而可能误读为"合规"。事实并非如
> 此：模块指标可以区分两种运行模式，但两个值都不能证明证书范围、来源或部署合规。

### 姿态告警

主机与二进制的姿态不匹配——FIPS 节点运行了未启用 BoringSSL FIPS 模式的二进制——可以用一条 PromQL
表达式捕获：

```promql
edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0
```

这正是启动期护栏 WARN 所检查的同一条件；该指标让你可以在整个集群范围内对其告警，而不必翻查 Pod 日志。

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

FIPS 模式覆盖了常见的下行姿态，但存在若干有记录的缺口（当前契约见
`skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md`）：

- **当前 Gateway 的密码套件列表策略不限制 TLS 1.3 套件。** `TLS_CHACHA20_POLY1305_SHA256` 在 TLS 1.3 上仍可协商。
  批准套件强制仅在 **TLS 1.2** 生效。若要彻底排除 TLS 1.3 ChaCha20，必须禁用 TLS 1.3（默认不禁用）。
- **上行（Gateway → 后端）TLS 密码套件不受 FIPS 限制。** Pingora 0.8.1 连接器使用静态密码套件列表，无
  per-peer 钩子。上行依赖 BoringSSL FIPS 模式加既有的服务端证书校验。
- **不强制密钥类型策略**（例如拒绝 Ed25519 叶子密钥）。
- **TLS 1.3 会话恢复（PSK）** 与完整 TLS 1.3 握手具有相同的密码套件限制：Gateway 使用的 TLS 1.2 密码套件列表
  API 不限制 TLS 1.3 套件。曲线策略在证书查找之前的早期 ClientHello 回调中单独应用。
  本次变更尚未验证 PSK 专属的曲线协商行为。

## 故障排查

| 现象 | 可能原因 | 处理 |
|------|----------|------|
| Gateway 启动时报 FIPS 错误并非零退出 | 非 FIPS 镜像上设了 `require_fips: true` | 部署 FIPS 构建镜像；若该节点不要求 FIPS，则去掉 `require_fips` |
| `edgion_gateway_fips_module_enabled` 为 `0` | 二进制不是 FIPS 构建 | 运行的镜像未以 `fips` feature 编译——改用 FIPS 镜像 |
| Gateway 配置未变，但 ChaCha20 或纯 CBC 的 TLS 1.2 客户端突然失败 | 节点变成了 FIPS 主机，且 `fips_policy` 处于 `auto` 默认值 | 为该部署设置 `fips_policy: off`，或为该工作负载设置 `EdgionTls.spec.ciphers` |
| ChaCha20 客户端在 TLS 1.2 上失败，但你以为能用 | FIPS 批准默认套件排除了 ChaCha20 | 在该 listener 显式设置 `EdgionTls.spec.ciphers`，或将这些客户端路由到非 FIPS listener |
| ChaCha20 在 TLS 1.3 上仍能协商 | 当前 Gateway 的密码套件列表策略仅控制 TLS 1.2 套件 | 排除 TLS 1.3 ChaCha20 需要单独修改 TLS 策略 |
| 本地（非 FIPS）构建 / 非 FIPS 主机上未应用批准策略 | `fips_policy` 解析为 `off`（或 `auto` 且主机非 FIPS） | 设置 `fips_policy: on`（或 `EDGION_FIPS_POLICY=on`）以在无 FIPS 模块时验证策略 |
| YAML 中 `fips_policy: true` 加载失败 | 历史遗留布尔写法；`fips_policy` 是字符串三态，不是布尔值 | 改用 `fips_policy: "on"` / `"off"` / `auto` |
| `edgion_gateway_fips_host_detect_error` 为 `1` | 主机检测无法读取 `/proc/sys/crypto/fips_enabled`（例如被加固 / 屏蔽的 `/proc`） | 显式设置 `fips_policy`（`on`/`off`）；检测处于 `Unknown` 时 `auto` 不生效 |
| 沙箱化运行时内 `edgion_gateway_fips_host_enabled` 的读数与预期相反 | Kata/Firecracker/gVisor：`/proc/sys/crypto/fips_enabled` 反映的是客户机内核，而非物理节点 | 在该运行时上显式设置 `fips_policy`；不要依赖 `auto` |

## 相关文档

- [EdgionTls](./edgion-tls.md)——密码套件 / 最低 TLS 版本控制与 `ciphers` 覆盖
- [TLS 终结](./tls-termination.md)
- [Gateway 配置](../../configuration/gateway-config.md)——`require_fips` / `fips_policy` 参考
- [监控指标](../../observability/metrics.md)——抓取 `edgion_gateway_fips_module_enabled`、`edgion_gateway_fips_host_enabled`、`edgion_gateway_fips_host_detect_error`、`edgion_gateway_fips_policy_active`
