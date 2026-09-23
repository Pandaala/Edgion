# Wasm 插件

> 从 Envoy 迁移时，可使用 `edgion-cli wasm inspect` 按 Edgion 精确 Host
> Profile 检查模块，并使用 `edgion-cli wasm convert-envoy` 将单个 v3 HTTP
> Wasm Filter 或裸 PluginConfig 严格转换为 `WasmConfig` 片段。参见
> [CLI 指南](../../../../ops-guide/edgion-cli.md#wasm---本地检查与-envoy-迁移)。
> 这是迁移助手，不是无缝兼容证明；用户仍须选择 EdgionPlugins stage，响应 body
> 仍保持逐 chunk 流式处理。Edgion 还会把 guest 可见的 `plugin_name` 固定为
> `Wasm`；即使 Envoy plugin name 为空，转换器也会报告差异 warning。

> **🔌 Edgion 扩展**
>
> Wasm 插件在网关进程内通过 [wasmtime](https://wasmtime.dev/) 直接执行 [proxy-wasm](https://github.com/proxy-wasm/spec) ABI guest 模块——一个用 Rust、C++、Go、TinyGo 或任意 proxy-wasm SDK 编译出的普通 WebAssembly 二进制文件。它由 `EdgionPlugins` CRD 提供，不属于标准 Gateway API。

## Wasm 解决什么问题？

proxy-wasm 是一套可移植的代理扩展 ABI：编写一次扩展，即可运行在任意兼容 host（Envoy、本网关或其他实现）上，无需重新编译 host 本身。你不需要针对 Edgion 内部 API 编写原生 Rust 插件，而是用你偏好语言的 [proxy-wasm SDK](https://crates.io/crates/proxy-wasm) 编译出一个体积较小的 WebAssembly 模块，把编译产物 `.wasm` 字节码交给本插件。每个插件运行时对其加载代次编译一次，并池化初始化后的实例供请求复用；插件重建可能再次编译。

实现的 ABI 是一个刻意收敛的子集（见下文）：header 读写、本地响应、**HTTP callout**（请求阶段 Pause/恢复，以及 `on_tick` 内发起）、**请求 body 整体缓冲模式**（完整 body、由 guest 声明、可拒绝或改写请求）、**响应 body 只读观测**（按 chunk）、**shared data** 与**定时器**（`on_tick`）。不支持完整响应体缓冲和响应 body 改写；主上游响应始终保持流式传输。

## 支持的 ABI 子集

### 钩子（Hooks）

| proxy-wasm 钩子 | EdgionPlugins 阶段 | 运行时机 |
|---|---|---|
| `proxy_on_context_create` / `proxy_on_vm_start` / `proxy_on_configure` | —（实例初始化） | 每个池化实例在服务任何请求之前执行一次 |
| `proxy_on_request_headers` | `requestPlugins`（`RequestFilter`） | 请求 header 到达、访问后端之前 |
| `proxy_on_request_body` | `requestPlugins`（`RequestFilter`），与 header 钩子同一条目 | `proxy_on_request_headers` **之后**每请求一次，仅当 guest 模块导出该钩子——见[请求 body 整体缓冲模式](#请求-body-整体缓冲模式) |
| `proxy_on_http_call_response` | —（callout 回调） | 每个完成的 callout 回调一次：请求阶段发起的在挂起的请求上下文上，`proxy_on_tick` 发起的在 root context 上 |
| `proxy_on_response_headers` | `upstreamResponseFilterPlugins`（`UpstreamResponseFilter`） | 上游响应 header 到达 |
| `proxy_on_response_body` | `upstreamResponseBodyFilterPlugins`（`UpstreamResponseBodyFilter`） | 每个上游 body chunk 一次，**只读**（见[响应 body 观测](#响应-body-观测只读)） |
| `proxy_on_tick` | —（后台） | guest 设定 tick 周期后周期性触发（见[定时器](#shared-data-与定时器)） |
| `proxy_on_done` / `proxy_on_delete` | —（上下文清理） | 每个请求级上下文结束之后 |

Wasm **不支持**（异步的）`upstreamResponsePlugins` 阶段。
如果模块需要完整请求 body，应将其放在 `requestPlugins`。`RequestBodyBuffer`
可为 DSL 等受支持的后续消费者保留请求 body，但响应阶段的 Wasm 宿主阶段不会暴露该缓存。

### Host 调用

| Host 函数 | 支持 | 说明 |
|---|:---:|---|
| `proxy_get_header_map_pairs` / `proxy_get_header_map_value` | ✅ | 请求头、响应头，以及（callout 回调内）`HttpCallResponseHeaders`——callout 响应头外加合成的 `:status` |
| `proxy_replace_header_map_value` / `proxy_add_header_map_value` / `proxy_remove_header_map_value` / `proxy_set_header_map_pairs` | ✅ | 设置/追加/删除/整体替换；钩子返回后应用到真实请求/响应。对 callout 响应 map 的写入被拒绝 |
| `proxy_send_local_response` | ✅ | 以指定状态码/响应体/头短路请求。响应体按字节原样透传，二进制内容（图片、protobuf 等）不会被改写；空响应体产生无 body 的响应。也可在 `proxy_on_http_call_response` 内调用（鉴权 callout 后拒绝） |
| `proxy_http_call` | ✅（请求头 + tick 阶段） | 见 [HTTP callout](#http-calloutpause恢复)。请求 body、响应头、响应 body 阶段均返回 `UNIMPLEMENTED`（12）；tick 外呼须由所属插件的 allowlist 授权（见[定时器](#shared-data-与定时器)） |
| `proxy_continue_request` / `proxy_continue_stream(Request)` | ✅ | 恢复因 callout 挂起的请求。`proxy_continue_response` 仍是接受型 no-op |
| `proxy_get_shared_data` / `proxy_set_shared_data` | ✅ | 插件运行时 KV + CAS——见 [shared data](#shared-data-与定时器) |
| `proxy_set_tick_period_milliseconds` | ✅ | 启动本插件实例的 tick 任务；`0` 停止。周期下限 100ms |
| `proxy_get_property` / `proxy_set_property` | ✅（基础子集） | 可读键：`request.path`、`request.method`、`request.host`、`source.address`、`plugin_name`（固定为 `Wasm`）、`response.code`（仅响应阶段）。`proxy_set_property` 仅写入当前阶段的内存属性表 |
| `proxy_get_buffer_bytes` | ✅ | `VmConfiguration`、`PluginConfiguration`、`HttpCallResponseBody`（callout 回调内）、`HttpRequestBody`（请求 body 钩子内的**完整缓冲 body**——见[请求 body 整体缓冲模式](#请求-body-整体缓冲模式)）、`HttpResponseBody`（响应 body 钩子内的**当前 chunk 窗口**） |
| `proxy_set_buffer_bytes` | ✅（仅请求 body） | 在完整请求 body 钩子内替换、插入、追加或删除 `HttpRequestBody` 字节。响应 body 写入返回 `STATUS_UNIMPLEMENTED`（12） |
| `proxy_log` | ✅ | Guest 文本仅在当前 hook 的有界内存诊断中暂存，随后丢弃；访问日志只记录有界行数，绝不记录 guest 控制的原文 |
| `proxy_get_current_time_nanoseconds` | ✅ | 系统时钟 |
| `proxy_get_configuration`（0.1 遗留） | ✅ | 返回与 `PluginConfiguration` buffer 相同的字节 |
| `proxy_set_effective_context` / `proxy_done` | 接受型 no-op | 返回 `STATUS_OK`，不做任何事 |
| SDK 标准声明但 Edgion 未实现的调用（gRPC callout、shared queue、foreign function、guest metrics、log-level 查询） | ⚠️ | SDK 即使不使用也可能声明，因此其精确标准签名可通过加载。兼容性报告会标为 `known_unsupported_trap` 并给出 warning；一旦调用就会 trap，并进入[失败语义](#失败语义) |
| 其余全部 | ❌ | 模块加载时由兼容性检查拒绝；即使 guest 从不调用，真正未知的 import 也不会被接受 |

### ABI 版本检查

完成来源完整性校验后、交给 Wasmtime 编译前，host 会解析 core Wasm 模块并检查 ABI 标记、import 与已识别的 hook export：

- `proxy_abi_version_0_2_0` / `proxy_abi_version_0_2_1`——接受。
- 任何不支持的标记，或同时导出多个 ABI 标记——**加载错误**。
- 完全没有标记——WARN 后放行（兼容手写测试模块与非标 guest）。
- 每个 import 都必须是来自 `env` 模块的 function，名称必须存在于当前 host 的精确 profile 中，参数和返回值类型必须精确匹配。
- 标准 Proxy-Wasm hook 名称必须是 function export，且精确匹配 SDK ABI 签名。其他任何 `proxy_on_*` export 都会按未知或拼写错误的 hook 拒绝；不相关的自定义 export 仍允许。WebAssembly component、畸形模块、非 function import、错误 import 模块、未知 import 与签名不匹配都会在 JIT 编译前被拒绝。
- 64MiB 来源字节上限与结构复杂度限制相互独立。完整校验前，分析器会拒绝以下任一情况：会建立 validator 索引的 core section（type、import、function、table、memory、tag、global、export、element、data）条目合计超过 100,000 项；import/export inventory 合计超过 50,000 项；import/export 名称字节合计超过 4MiB；function type 签名单元合计超过 100,000；或报告需要实例化的签名单元超过 100,000。展开后的 local 每个 function 最多 100,000 个、每个模块合计最多 1,000,000 个；单个 function 编码 body 上限为 256KiB，以限制 validator 对单个 function 的展开工作。此时返回 `analysis_limit_exceeded`；不会截断报告后仍声称兼容。

兼容性失败统一使用一条有界的通用加载错误；网关不会把攻击者可控的 import/export 名称或 parser 细节复制到请求可见的失败文本。

兼容性报告中的 `compatible: true` 表示**可按当前 host profile 安全加载**，不表示每项已声明功能都已实现，也不表示每个导出的 hook 都会被调用。请同时检查 import 与 hook 的 `support` 字段和 warning 列表。`known_unsupported_trap` import 可以保持未调用，但调用时会 trap。SDK 经常默认导出的标准 hook 若标为 `known_not_invoked`（包括请求/响应 trailer、queue-ready、gRPC 回调、foreign-function 回调及 stream 回调），可以通过加载，但 Edgion 永远不会调用；标为 `implemented` 的 hook 不会产生此 warning。

## 配置参考

```yaml
type: Wasm
config:
  source:
    file: ./plugins/header_demo.wasm
  pluginConfig:
    json:
      add_header: x-wasm-added
  failOpen: false
  timeoutMs: 100
```

| 参数 | 类型 | 默认值 | 说明 |
|-----------|------|---------|-------------|
| `source.file` | String | — | 本地 `.wasm` 文件路径，相对路径基于网关工作目录解析。`file` / `inline` / `url` / `oci` 四者必须恰好设置一个。 |
| `source.inline` | String (base64) | — | base64 编码、内嵌在资源里的字节码。**仅作为开发/测试便利**——解码后硬上限 1MiB（超出为校验错误），超过 512KiB 网关在加载时 WARN。见[来源形态选型](#来源形态选型)。 |
| `source.url` | String | — | 网关拉取字节码的 `http://` / `https://` URL。**必须配 `sha256`。** 连接行为可通过 `source.fetch` 调整。 |
| `source.fetch` | Object | — | `url` 来源的可选拉取强化（与 `file`/`inline`/`oci` 同用为校验错误）：TLS、认证、超时、重试。 |
| `source.fetch.tls` | Object | — | 共享 `TlsConfig` 结构：自定义 CA（`validation.caCertificateRefs`）和/或 mTLS 客户端身份（`clientCertificateRef`）。Controller 会解析 Secret 和 ConfigMap CA 引用（跨 namespace 时需要 kind 匹配的 `ReferenceGrant`）；ConfigMap 缺失时报告 `ResolvedRefs=False/ConfigMapNotFound`。 |
| `source.fetch.authHeaderSecretRef` | Object | — | `{name, namespace?, key}`：Secret 的 `key`（默认 `authorization`）保存拉取时发送的完整 `Authorization` 头值（如 `Bearer <token>`）。设置了引用但 Secret 尚未解析时，加载**失败**而不是静默匿名拉取。 |
| `source.fetch.timeoutMs` | Integer | `10000` | 单次尝试超时。范围 `1..=60000`。 |
| `source.fetch.retry` | Object | — | `{attempts, backoffMs}`——含首次的总尝试数（`1..=10`，默认 3）与逐次翻倍的基础退避（默认 500，上限 30000）。策略固定：传输错误与 `5xx` 重试；**`4xx` 永不重试**。 |
| `source.oci` | Object | — | 从 OCI 镜像仓库拉取模块：`{image, pullPolicy?, pullSecretRef?}`。见[`source.oci`——从 OCI 镜像仓库拉取](#sourceoci从-oci-镜像仓库拉取)。 |
| `sha256` | String (64 hex) | — | 原始字节码的 SHA-256，编译前校验。`file` / `inline` / `oci` 可选；`url` **必填**。 |
| `pluginConfig` | 原始字节对象 | — | 传给 `proxy_on_configure` 的不透明字节，经 `PluginConfiguration` buffer 可读。见[Guest 配置字节](#guest-配置字节)。 |
| `vmConfig` | 原始字节对象 | — | 传给 `proxy_on_vm_start` 的不透明字节，经 `VmConfiguration` buffer 可读。见[Guest 配置字节](#guest-配置字节)。 |
| `failOpen` | Boolean | `false` | 加载失败 / trap / 超时 / body 访问失败 / callout 不恢复时的行为——见[失败语义](#失败语义)。 |
| `timeoutMs` | Integer | `100` | 每次钩子调用的墙钟预算（epoch 中断）。范围 `1..=10000`。body chunk 钩子按 `timeoutMs / 10` 执行。 |
| `instancePoolSize` | Integer | `8` | 每个编译模块、每个网关进程保留的池化实例上限。范围 `1..=64`。 |
| `calloutAllowlist` | String 列表 | `[]` | guest 可通过 `proxy_http_call` 访问的 origin（`scheme://host[:port]`）。**空 = 禁用 callout**（dispatch 返回 guest 可观测的宿主错误）。规范化后精确匹配。 |
| `calloutTimeoutMs` | Integer | `5000` | 每个 callout 的 guest 超时上限（guest 传 0 时的生效值）。范围 `1..=10000`。 |

### Guest 配置字节

`pluginConfig` 与 `vmConfig` 各自必须且只能使用以下一种对象形态：

```yaml
pluginConfig:
  string: "plain UTF-8"
vmConfig:
  base64: "/wA=" # 可表达包括非 UTF-8 在内的任意字节
```

结构化数据使用 `{json: <value>}`。它允许任意 JSON value，包括 object、array、
string、number、boolean 与 null。Edgion 将解析后的 value 编码为紧凑 UTF-8 JSON，
并递归排序 object key。guest 收到的正是这组 canonical 字节；
原始空白、key 顺序与 number 写法不会保留。

三种形态严格互斥。缺少 selector、同时设置多个 selector、未知字段、旧式裸 string、
非 canonical 的 Standard padded base64，以及解码后超过 64KiB 的值都会被拒绝。
字段缺省与显式 empty 的语义不同；不同形态若产生相同字节，guest 收到的配置等价，
但分别配置的插件条目仍拥有独立运行时。

## 来源形态选型

| 形态 | 定位 | 大小上限 | 分发路径 |
|---|---|---|---|
| `inline` | **开发/测试便利**，不是生产分发机制 | 解码后 1MiB（硬上限）；另受 K8s 对象/etcd 限制 | 随 CR 进 etcd；每次 conf_sync 全量携带 |
| `file` | 打进网关镜像或 initContainer 挂载的模块 | 64MiB 硬上限 | 镜像构建 / volume 挂载 |
| `url` | **生产推荐**——内部制品库（Nexus、Artifactory、S3 presigned） | 64MiB 硬上限 | 网关直拉，配合 `source.fetch` 的 TLS/认证/重试与强制 `sha256` |
| `oci` | **生产推荐**——对齐 Istio 的 OCI 镜像分发 | blob 与解压后模块均为 64MiB 硬上限 | 网关直接从 OCI 镜像仓库拉取，配合内容寻址的磁盘缓存 |

这与 Istio 的取向一致：Istio 的 `WasmPlugin` **根本没有 inline 字段**，以 `oci://` 为主通道、`http(s)` 次之。Edgion 保留 inline 用于快速迭代，但加上限并告警；ConfigMap 形态经评估后否决（与 inline 同受 etcd 对象上限，还引入 ref 追踪复杂度，无增量价值）。

### `source.oci`——从 OCI 镜像仓库拉取

```yaml
source:
  oci:
    image: registry.internal/wasm/header-demo:1.2.0   # 或 ...@sha256:<digest>
    pullPolicy: IfNotPresent                          # 可选：IfNotPresent | Always
    pullSecretRef:                                    # 可选，私有仓库使用
      name: registry-creds                            # kubernetes.io/dockerconfigjson 类型 Secret
```

| 字段 | 说明 |
|---|---|
| `image` | `registry/repo:tag` 或 `registry/repo@sha256:<digest>`。registry host 部分必填。 |
| `pullPolicy` | 不设置时遵循 Kubernetes 惯例：`latest` 或缺省 tag 为 `Always`，其余为 `IfNotPresent`。digest 引用永远视为已固定。`Always` 最多每 60s 重新解析一次 tag 对应的 digest，且仅在 digest 变化时才重新加载。 |
| `pullSecretRef` | `kubernetes.io/dockerconfigjson` 类型 Secret。跨 namespace 引用需要 ReferenceGrant。如果 Secret 无法解析，插件加载**失败**——不存在静默匿名回退。 |

支持的镜像形态（对齐 Istio）：媒体类型为
`application/vnd.module.wasm.content.layer.v1+wasm` 的层（由 `oras` 推送），或
一个包含 `plugin.wasm` 的单个 tar+gzip 层（普通 `docker build` 即可产出）。
OCI 来源的 `sha256` 仍是可选项——digest 链路本身已覆盖传输完整性——若设置，
会像其他来源一样对最终 wasm 字节做校验。

通过摘要校验的 manifest 与 blob 按内容摘要缓存在进程内存中。默认保留预算为
256 MiB、最多 256 个条目；容量不足时按 LRU 淘汰，闲置 24 小时过期，后台每
60 秒清理一次。将 `wasm_oci_cache_max_bytes` 或 `wasm_oci_cache_max_entries`
设为 `0` 可禁用缓存保留。预算不足时可以跳过缓存，不影响有效模块的本次加载。

OCI 不再使用磁盘缓存或缓存目录配置。网关重启后需要重新下载制品，包括按
摘要固定的镜像。旧版本留下的缓存文件不会被读取或自动删除。最后一个 Wasm
插件移除时会清空制品缓存；正在使用的编译模块及在途请求持有的模块引用不受影响。

字节预算统计载荷的实际分配容量，包括已从缓存淘汰、但仍被加载任务引用的字节。
它不是全部 Wasm 或进程内存的上限：最多四条 OCI 加载流水线还可持有受大小限制的
下载及解压缓冲区，编译结果、VM 实例和 guest 内存另有生命周期。缓存命中共享
原始字节，不复制整个载荷。manifest、blob、解压模块和令牌响应的大小上限分别
保持为 4 MiB、64 MiB、64 MiB 和 64 KiB。

相关网关配置字段为 `wasm_oci_cache_max_bytes`、`wasm_oci_cache_max_entries`、
`wasm_oci_cache_ttl_secs`、`wasm_oci_cache_purge_interval_secs`、
`wasm_oci_pull_timeout_secs` 和 `wasm_oci_pull_attempts`。条目上限超过 65,536
会被拒绝；TTL 或清理周期为零时在启动阶段调整为一秒。拉取超时及重试次数
独立于缓存容量限制。

限制（v1）：镜像仓库必须提供可公开验证的 TLS 证书（本地测试用的 loopback
仓库可使用明文 HTTP）；不支持镜像仓库镜像（registry mirror）。对于静态部署，
`source.file` 搭配 initContainer（或 Kubernetes 的 `image` volume source）
仍是 kubelet 原生的替代方案——`source.oci` 面向需要不重启 Pod 就热加载的
动态下发插件场景。

### url + 拉取强化（生产示例）

```yaml
requestPlugins:
  - type: Wasm
    config:
      source:
        url: "https://artifacts.internal/wasm/header-demo-1.2.0.wasm"
        fetch:
          tls:
            enabled: true
            validation:
              caCertificateRefs:
                - { kind: Secret, name: artifact-repo-ca }
          authHeaderSecretRef:
            name: artifact-repo-token
            key: authorization        # Secret 值形如 "Bearer eyJ..."
          timeoutMs: 10000
          retry: { attempts: 3, backoffMs: 500 }
      sha256: "3b1c...e4"
```

说明：

- 重试仅针对传输错误与 `5xx`；`401`/`403`/`404` 立即失败（认证错误重试无意义，且会刷爆制品库日志）。
- 全部尝试失败按一次加载失败计，与其他加载错误一样进入 30s 负缓存。
- 解析出的 `Authorization` 值在 admin `/configclient` 输出中显示为 `[redacted]`。
- 不同 URL 的并发拉取全进程上限 4（防启动风暴）；同 URL 的并发调用本就共享一次在途加载。

## HTTP callout（Pause/恢复）

`proxy_http_call`（官方 proxy-wasm SDK 实际导入的名字；host 同时兼容旧的 `proxy_dispatch_http_call` 拼写）允许 guest 在请求中途调用外部 HTTP 服务——proxy-wasm 生态最主流的插件形态（外部鉴权、风控评分、审计）。流程：

1. guest 在 `on_http_request_headers` 里 dispatch 一或多个 callout，返回 `Action::Pause`。
2. 网关在请求挂起期间（异步、不占 guest CPU 预算）执行这些 callout。
3. 每个结果经 `proxy_on_http_call_response` 投递；guest 读取 `HttpCallResponseHeaders` / `HttpCallResponseBody`，然后要么恢复请求（`resume_http_request` / `proxy_continue_request`），要么直接应答（`send_http_response` / `proxy_send_local_response`）。

语义与护栏：

- **寻址**：`upstream` 参数是目标 **origin URL**（`https://auth.internal:8443`），不是 Envoy 的 cluster 名。dispatch 的 header map 里的 `:path`、`:method`、`:authority` 伪头驱动外呼请求（`:authority` 作为 `Host` 发送）。
- **允许列表**：origin 必须（规范化后）精确匹配 `calloutAllowlist` 的某一项。没有允许列表 = 没有 callout。被拒绝的 dispatch 同步返回 `INTERNAL_FAILURE`。
- **请求 + tick 阶段**：响应头/body 阶段是同步的、无法 await，dispatch 返回 `UNIMPLEMENTED`。`on_tick` 内的 callout 使用所属插件的授权，回调投递到 root context，见[定时器](#shared-data-与定时器)。
- **必须 Pause**（请求阶段）：`dispatch` + `Action::Continue` 不受支持——排队的 callout 被丢弃并记日志（`callouts-dropped=N`）。dispatch 之后必须 Pause。（`on_tick` 没有 Pause 概念：在那里 dispatch 的 callout 总是在钩子返回后执行。）
- **预算**：每请求（或每 tick）最多 8 次 dispatch、4 轮回调（回调里可以追发 callout）、全进程 64 个并发 callout、单 callout 响应体上限 256KiB。每个 callout 的超时为 `min(guest 超时, calloutTimeoutMs)`。
- **callout 失败**（连接错误、超时、响应体超限）：回调以 **0 个 header** 触发（proxy-wasm 惯例），guest 自行决定失败行为。宿主绝不因 callout 失败而 trap guest。
- **guest 不恢复**：所有 callout 完成后 guest 既没 continue 也没本地应答 → 宿主按 `failOpen` 处理（reason `no-resume`）。
- **epoch 预算**：`timeoutMs` 只约束每次 *guest* 钩子调用；callout 的网络等待发生在 guest 调用之间，不消耗 guest CPU 预算。

### callout 与 ExtProc 的分工

两者都是接入外部决策服务，方向不同：

- **ExtProc**（推模式）：网关把每个请求各阶段推给常驻外部处理器（gRPC）。外部服务拥有逻辑、需要看到全部流量时选它。
- **Wasm callout**（拉模式）：guest 逻辑内嵌网关，按需外呼。插件自己决定*是否/何时*咨询外部服务时（如先查缓存、未命中再校验 token）选它。

## 两种 body 模式，静态选择

Edgion 支持两种、有意区分开的 wasm body 行为：

| | 请求 body 整体缓冲模式 | 响应 body 观测 |
|---|---|---|
| 钩子 | `proxy_on_request_body` | `proxy_on_response_body` |
| 所在条目阶段 | `requestPlugins`（与 header 钩子同一条目） | `upstreamResponseBodyFilterPlugins` |
| `body_size` | **完整 body** 长度 | **当前 chunk** 长度 |
| `proxy_get_buffer_bytes` 返回 | **完整缓冲 body** | 仅**当前 chunk** |
| 能否拒绝请求 | ✅（`proxy_send_local_response`） | ❌（响应头已发出） |
| 能否改写 body | ✅（`proxy_set_buffer_bytes`） | ❌ |
| 如何选择 | 静态：编译出的 guest 模块导出了 `proxy_on_request_body`；可选的 `body:` 块用于添加执行前 body 策略 | 静态：guest 模块是否导出 `proxy_on_response_body` |

**这是对 proxy-wasm spec 的有意偏离。** spec 把缓冲建模为 guest 在运行时逐 chunk 做的*动态*决策（`if !end_of_stream { Action::Pause }` 告诉宿主继续缓冲；`Action::Continue` 告诉宿主继续流式转发）。Envoy 的实现方式是把 `Pause` 映射成 `StopIterationAndBuffer`，由它的 filter manager 持有缓冲区。Edgion 在两侧都做不到这一点——响应 body filter 是**同步**、body 不可变的直接传递（入参 `&Option<Bytes>`，返回 `Option<Duration>`；无法扣留或累积某个 chunk），而且"这个插件是否拿到完整 body"必须在任何钩子运行之前、模块加载时就决定一次。所以 Edgion 只在**一次、静态**地选择模式：依据编译出的 guest 模块导出了哪个钩子、以及运营者把它挂在哪个阶段——而不是依据 guest 每次请求返回的 `Action` 值。

### 请求 body 整体缓冲模式

Wasm 条目列在 `requestPlugins` 下且 guest 导出了 `proxy_on_request_body` 时，就会进入完整请求体模式。没有 `body:` 块时，`proxy_on_request_headers` 返回 `Continue`（即没有已经拒绝或 trap）之后，网关才会懒捕获完整的请求 body。随后网关调用 `proxy_on_request_body` **恰好一次**：`body_size` 为完整 body 的长度、`end_of_stream = true`，`proxy_get_buffer_bytes(HttpRequestBody, 0, body_size)` 返回完整内容。guest 可以检视完整 body——用于请求体 WAF、敏感信息扫描或签名校验——像在 header 钩子里一样通过 `proxy_send_local_response` 拒绝请求，或通过 `proxy_set_buffer_bytes(HttpRequestBody, start, size, bytes)` 改写请求。

需要注意的现状与限制：

- **通过可选的 `body:` 添加策略门控。** 导出的钩子会开启请求捕获；`body:` 则在 Wasm 请求头钩子之前添加通用门控。它的 `maxBodySize` 是叠加在网关物理上限之上的**单插件策略限制**，不是缓冲区分配大小：超过限制时，默认的 `onReadFailure: failClose` 返回 `413`，而 `failOpen` 会跳过整个 Wasm 条目。`conditions` 决定是否执行这层前置门控策略；条件要求跳过时，内部 Wasm 仍会因为导出的钩子而稍后捕获 body。如果 guest 没有导出 `proxy_on_request_body`，配置 `body:` 仍会在请求头钩子之前捕获，但不会运行 body 钩子。
- **上限**：缓冲受已解析的 `EdgionGatewayConfig.spec.requestBody` 策略约束，不是某个插件专属的分配值。
- **共享且有界的捕获。** 该模式复用其他完整请求体插件使用的请求级 body 存储。捕获是懒执行的，受网关物理上限约束，超过内存阈值后会溢写磁盘，并向 guest 暴露此前插件产生的当前不可变 body 版本。网关物理上限和不符合缓冲条件的请求始终 fail-close。
- **严格的范围语义。** `[start, start + size)` 必须完全包含在当前暂存 body 内。长度为零的范围表示插入；`start == body length` 且 `size == 0` 表示追加；替换字节为空表示删除所选范围。无效范围返回 `BAD_ARGUMENT`，超过物理 body 上限返回 `INTERNAL_FAILURE`。
- **有界的修改工作量。** 每次请求 body 钩子的累计修改工作量上限，为已解析网关物理 body 上限的两倍。每个操作计入替换字节；如果操作改变长度，还会计入必须搬移的尾部字节。该预算允许正常的范围修改，以及在任意已配置 body 上限下最多两次等长完整重写，同时阻止反复整体重写或在开头循环插入/删除放大 CPU 和内存带宽消耗；物理上限较小时，该预算也会同比缩小。超过预算的操作返回 `INTERNAL_FAILURE`，且不会改变暂存 body。
- **SDK 错误行为。** host 返回上述 ABI 状态时不会主动 trap guest，但部分高级 proxy-wasm SDK helper（包括 Rust 的 `set_http_request_body`）会在 host 调用返回非 OK 状态时 panic；使用此类 helper 时应在 guest 内先校验范围和输出大小。
- **原子发布。** 同一钩子内的所有成功写入先更新私有暂存区；被拒绝的 host 调用不会由该次调用改变暂存区。继续转发的钩子返回后，网关才把最终字节一次性发布到共享请求 body 存储；trap 或发布失败不会暴露发布到一半的 body。本地响应已经终止代理请求，因此跳过发布。后续插件、请求镜像、重试和上游重放均读取已提交的同一代 body；上游 framing 按最终长度重新计算。
- **捕获时机取决于是否配置 `body:`。** 配置后，通用门控会先捕获并应用策略，再运行任何 Wasm 钩子；未配置时，先运行请求头钩子，只有干净的 `Continue` 才继续懒捕获并运行 body 钩子。两条路径中的 body 钩子看到的是同一个完整请求版本。未导出 `proxy_on_request_body` 的 guest 会完全跳过 body 阶段。

请求 body guest 的最小形态：

```rust
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;

impl HttpContext for BodyScanner {
    fn on_http_request_headers(&mut self, _n: usize, _eos: bool) -> Action {
        Action::Continue // 放行到 body 钩子
    }

    // 只会被调用一次，拿到的是完整请求 body（Edgion 已经帮你缓冲好了——
    // 不需要经典的 `if !end_of_stream { Pause }` 那套动作）。
    fn on_http_request_body(&mut self, body_size: usize, _end_of_stream: bool) -> Action {
        if let Some(body) = self.get_http_request_body(0, body_size) {
            if body.windows(6).any(|w| w == b"secret") {
                self.send_http_response(403, vec![], Some(b"request body denied"));
            }
        }
        Action::Continue
    }
}
```

### 响应 body 观测（只读）

把 Wasm 条目列在 `upstreamResponseBodyFilterPlugins` 下，`proxy_on_response_body` 会对每个上游 chunk 触发一次。这是响应侧**唯一**可用的 wasm body 模式：主上游响应始终保持流式传输，不提供完整响应体缓冲模式（见[剩余限制](#剩余限制)）。

> ### ⚠️ 按 chunk 而非整体——移植 guest 前必读
>
> `body_size` 是**当前 chunk** 的长度，`proxy_get_buffer_bytes(HttpResponseBody, …)` **只返回该 chunk**，body 从不跨 chunk 累积。
>
> **后果**：一个按动态缓冲范式写的 guest——
> ```rust
> if !end_of_stream { return Action::Pause; }             // 期望宿主缓冲
> let body = self.get_http_response_body(0, body_size)?;  // 以为拿到完整 body
> ```
> ——**只会看到最后一个 chunk**。对 WAF 或敏感数据扫描器来说这是**静默漏检**。因此每次无法兑现 `Pause` 时，宿主都会递增 `edgion_wasm_body_pause_unsupported_total{module}`。该有界指标是每 chunk 数据面路径上的唯一信号；不会再按模块输出 WARN。**请对该指标配告警**——它能可靠表明线上某个 guest 期望完整 body 缓冲。完整 body 模式只在*请求*侧可用，由导出的 `proxy_on_request_body` 钩子选择；可选的 `body:` 块用于添加策略。主上游响应始终仅支持流式处理。

在每 chunk 模型下正确的用法：

- 跨 chunk 聚合是 **guest 自己的事**（用它自己的内存，或 [shared data](#shared-data-与定时器)——参考 guest 就是把每个 chunk 的字节数累加进一个 shared 计数器）。
- **严格只读**：不能改写 body，也不能在 body 阶段短路响应（响应头已发出）。检测结论只能记日志（`proxy_log`）或写入 shared data。
- 每个 chunk 在缩减的 CPU 预算（`timeoutMs / 10`，下限 10ms）下执行，慢 guest 拖不垮长流；body 钩子 trap **绝不断流**（实例被丢弃，流继续，与 `failOpen` 无关）。
- 未导出 `proxy_on_response_body` 的 guest 在此阶段是零成本 no-op。

## Shared data 与定时器

### Shared data

`proxy_get_shared_data` / `proxy_set_shared_data` 提供带 proxy-wasm CAS 语义的插件运行时 KV（每 key 单调递增的 CAS 令牌；`set` 携带非零期望 CAS 且不匹配时返回 `CasMismatch`；CAS 0 无条件写；空值删除 key）。

- **作用域：单个网关进程内的一个配置插件实例。** 该实例的池化 VM 和 tick 跨请求共享数据。通用插件层会在同一配置代次中复用同一条目的插件实例；不同条目、不同阶段和重建后的配置代次各自拥有独立 store，即使字节码与配置完全相同。集群级协调使用 `rate_limit_redis`。
- **授权隔离：** tick 只使用所属插件的 allowlist；其他条目无法读取其 shared data，也不能为它增加可访问地址。
- 上限：单值 64KiB，每模块总量 4MiB。超限写入返回 `BAD_ARGUMENT`。总量按 key + value 字节**外加每条目约 128B 固定开销**记账（近似真实内存占用），因此海量小 key 场景可用的逻辑空间相应减少——该上限约束的是实际内存，而不只是负载字节。
- 生命周期：加载代次的最后一个持有者释放后回收，包括尚未结束的请求租约（见[插件运行时生命周期](#插件运行时生命周期)）。插件重建或 OCI 模块刷新从空 store 开始；新旧 store 可以短暂并存。

### 定时器（`on_tick`）

guest 调用 `proxy_set_tick_period_milliseconds(N)`（通常在 `on_configure` 里）后，后台任务（每个插件运行时一个）每 `N` ms（下限 100ms）调用一次 `proxy_on_tick`。tick 实例从与请求相同的池租借，受正常 `timeoutMs` epoch 预算约束，且**相邻两次 tick 可能落在不同的池化实例上**——root context 的结构体字段是实例本地的，需要跨 tick 存活的状态请放 shared data（在 `on_configure` 里*初始化*的状态没问题，比如解析好的配置：每个池化实例初始化时都会跑 `on_configure`）。guest 可在 `on_tick` 内调整周期或停止（`0`）。所属插件实例释放或加载的模块被替换时，tick 任务会被取消；正在执行的同步钩子仍受 epoch 超时约束。

典型模式：`on_tick` 发 callout 拉取最新规则集写进 shared data，请求钩子读取（限流规则热刷新、风控名单同步、配置轮询）。

#### `on_tick` 内的 callout

`on_tick` 可以像请求阶段一样 dispatch HTTP callout，差异在于：

- **没有 Pause/恢复**：`proxy_on_tick` 没有 Action 返回值；钩子期间排队的 callout 在钩子返回后立即执行，每个结果经 `proxy_on_http_call_response` 投递到 **root context**（与实例同生命周期——不涉及任何请求级上下文）。
- **插件独立授权。** tick 使用本插件实例的 `calloutAllowlist` 和 `calloutTimeoutMs`。空 allowlist 禁用本插件的 tick 外呼；使用相同字节码的其他插件既不能授予权限，也不会产生冲突。
- 超时由本插件的 `calloutTimeoutMs` 限制；其余护栏（每 tick 8 次 dispatch、4 轮、并发上限和响应体上限）不变。

## 失败语义

所有失败模式共用同一个 `failOpen` 决策：

| Reason | 触发条件 | PluginLog |
|---|---|---|
| `load` | 模块下载、解码、兼容性检查或编译失败，`sha256` 不匹配，或实例初始化失败（`on_vm_start`/`on_configure` 返回 false）。加载错误缓存 30s。 | `wasm load:fail-open` / `fail-close` |
| `acquire` | 请求时实例池无法产出可用 VM（如 pooling allocator 槽位耗尽）。 | `wasm acquire:…` |
| `trap` | guest panic、`unreachable`、CPU 预算超限，或在钩子（含 callout 回调与上下文关闭）中出错。 | `wasm trap:…` |
| `callout` | callout 解析结构性失败（如 guest 回调在投递中 trap）。 | `wasm callout:…` |
| `no-resume` | guest 因 callout 挂起，但全部完成后既未恢复也未应答。 | `wasm no-resume:…` |
| `body read` | 完整请求 body 在允许捕获后读取失败。阶段拒绝和捕获存储失败仍无条件 fail-close。 | `wasm body read:…` |
| `body mutation` | 发布 guest 最终暂存的请求 body 失败；其 body 与请求 header 操作都不会部分提交。 | `wasm body mutation:…` |
| `not loaded` | 仅响应阶段：同步钩子用非阻塞缓存查询；模块（异步的）首次加载未完成时触发，同时踢一次后台加载。 | `wasm not loaded:…` |

- **`failOpen: false`**（默认）——请求以 `500` + `wasm plugin error` 短路。
- **`failOpen: true`**——记录失败，插件链继续。

### trap 污染与恢复

trap（或其他故障）的 VM 实例会被**污染**：直接丢弃而不回池，释放其槽位。下一个请求从缓存的编译模块重新实例化（便宜——只有 `Module::new` 昂贵）。guest panic 绝不会 crash、挂起或损害网关进程。

### 超时

每次钩子调用由 `timeoutMs` 经 wasmtime **epoch 中断**约束：后台 ticker 每 10ms 递增引擎 epoch，deadline 在*每次* guest 调用（header 钩子、callout 回调、body chunk、tick）开始时重新武装。有效粒度约 10-20ms。callout 的网络等待发生在 guest 调用之间，不占 guest 预算。

### 宿主侧资源上限

`timeoutMs` 约束的是执行*时间*，而非宿主*内存*。另有两道上限约束单次钩子能在宿主侧分配的内存，防止有 bug 或恶意的 guest 把进程推向 OOM（OOM 会波及无关的路由与租户——`failOpen` 无法从全进程内存耗尽中恢复）：

- **每阶段 ABI 保留预算**——guest 在单次钩子内通过反复 ABI 调用累积的宿主保留量（排队的 header 操作、`proxy_log` 日志行、`proxy_set_property` 条目）上限为 **32 MiB / 20 万次操作**。超预算后后续 ABI 调用返回错误状态且**不做任何部分变更**；这些累积器在钩子返回时清空。合法的 header/日志/属性用量远低于此。
- **guest 线性内存上限**——guest 内存增长上限 **128 MiB**（表 10 万项），远高于任何真实 proxy-wasm 工作集。这同时约束了 guest 传给宿主调用的任何单个值的大小。

## 插件运行时生命周期

每个配置中的 Wasm 条目拥有自己的加载模块、实例池、shared data 和后台任务。对同一插件实例的引用通过通用插件层共享该运行时。

- **配置替换。** 重建条目会创建新的运行时。不同条目和阶段不共享可变状态，即使来源与配置相同。在途请求可以使用捕获的旧插件代次完成处理。
- **自动回收。** 最后一个插件持有者释放时，取消预加载、刷新和 tick，释放已发布模块。尚未结束的租约可以继续持有旧模块。没有全局运行时注册表或 60 秒模块回收宽限；`edgion_wasm_module_released_total` 统计已释放的加载代次。
- **重载代价。** 重建可能重新编译并重置 shared data。进程级引擎和有界 OCI 原始字节缓存继续共享。新旧代次允许短暂交叠，不保证配置替换前后的状态连续。
- **文件替换。** 覆写 `.wasm` 文件后，所属插件重建并重新加载文件时生效。单纯覆写不会替换已经加载的运行时。
- **预加载与刷新。** 预初始化可在首个请求前启动 tick。首次加载失败会进入负缓存。OCI `Always` 刷新失败保留最后成功版本；刷新成功取消旧 tick 并启动新代次。
- **容量规划。** 所有运行时使用共享引擎的 pooling allocator。容量需覆盖全部插件条目及短暂并存的新旧代次；`edgion_wasm_modules_loaded` 统计存活的编译运行时代次。

## 指标

按来源统计的指标带 `module` label；`edgion_wasm_modules_loaded` 是进程级指标。文件来源使用 `file:<path>`，内联来源使用 `inline:<sha8>`，URL 来源使用 `url:<canonical-origin>#<path-sha8>`。URL label 最长 96 字节，不含 userinfo、明文路径、查询参数或片段；仅签名查询凭据变化时，label 保持稳定。真实 URL 仍用于内部下载和缓存身份，不会进入指标或模块加载错误日志。

| 指标 | 类型 | Labels | 含义 |
|---|---|---|---|
| `edgion_wasm_module_load_total` | counter | `result=ok\|error` | 加载尝试（负缓存命中不计） |
| `edgion_wasm_module_compile_duration_ms` | histogram | | `Module::new` 耗时 |
| `edgion_wasm_module_released_total` | counter | | 最后一个持有者退出后释放的加载运行时代次数 |
| `edgion_wasm_instance_instantiate_total` | counter | | 新实例化（池 miss） |
| `edgion_wasm_instance_discard_total` | counter | `reason=poisoned\|over_capacity` | 未回池而被丢弃的实例 |
| `edgion_wasm_pool_size` | gauge | | 该来源标签最后上报的池内实例数，不汇总独立运行时 |
| `edgion_wasm_hook_duration_ms` | histogram | `phase` | guest 钩子耗时（`request_headers`、`request_body`、`response_headers`、`response_body`、`http_call_response`、`tick`） |
| `edgion_wasm_trap_total` | counter | `phase` | trap / 协议违规 |
| `edgion_wasm_fail_mode_total` | counter | `reason`、`mode=open\|close` | fail-mode 决策 |
| `edgion_wasm_callout_total` | counter | `result=ok\|error\|denied\|over_limit` | 所属插件授权下的 callout 结果 |
| `edgion_wasm_modules_loaded` | gauge | | 存活的编译运行时代次数，包括初始化中及旧租约仍持有的代次 |
| `edgion_wasm_shared_data_bytes` | gauge | `module` | 同一模块标签下所有存活 store 的 shared-data 字节总和，包括同时存活的新旧代及不同配置变体；计入 key、value 和每条目约 128B 开销。4MiB 上限针对单个 store，汇总值可以超过它。最后一个 store 释放后归零，零值序列之后可按配置的空闲超时被淘汰。 |
| `edgion_wasm_body_pause_unsupported_total` | counter | `module` | **建议配告警。** guest 在**响应 body 观测**钩子里返回了 `Action::Pause`——它想要整体 body 缓冲，而响应侧目前不提供该能力（只有请求侧的[请求 body 整体缓冲模式](#请求-body-整体缓冲模式)提供）——因此它正在静默地只看到当前 chunk |

trap 风暴表现为 `edgion_wasm_trap_total` 与 `edgion_wasm_instance_discard_total{reason="poisoned"}` 同步上涨。

## `PluginLog` 格式

- `wasm ok ops=N logs=M`——成功；应用了 `N` 个 header 操作，guest 输出 `M` 行日志（内容进 debug 日志）。
- `wasm ok ops=N pause-ignored`——guest 返回 `Pause` 但既没 dispatch callout 也没发本地响应；请求继续。
- `wasm callouts=N leftover=M`——请求阶段解析了 `N` 个 callout（`M` 个被轮次上限丢弃）。
- `wasm callouts-dropped=N`——guest dispatch 了 callout 却返回 `Continue`，被丢弃（不支持的用法）。
- `wasm local <status>`——guest 发送了本地响应。
- `wasm <reason>:fail-open` / `wasm <reason>:fail-close`——上文失败原因之一。

## 剩余限制

- **请求 body 整体缓冲模式由导出的钩子选择**：共享运行时会在网关物理上限约束下懒捕获完整请求。可选的 `body:` 块用于添加执行前条件、单插件 `maxBodySize` 和 `onReadFailure` 处理，而不是捕获开关。见[请求 body 整体缓冲模式](#请求-body-整体缓冲模式)。
- **响应侧没有整体缓冲模式**：主上游响应始终保持流式传输。Wasm 响应 body 观测仅按 chunk、只读——不能改写 body，不能在 body 阶段短路，`Action::Pause` 无法兑现——因此按标准整体缓冲范式写的 guest 只会看到当前 chunk。见[响应 body 观测](#响应-body-观测只读)一节的警告。
- **仅支持请求 body 改写**：`proxy_set_buffer_bytes(HttpRequestBody, ...)` 仅在完整请求 body 钩子内有效。响应 body 写入仍返回 `STATUS_UNIMPLEMENTED`，其他阶段返回明确的 ABI 错误。
- **静态选模式，非动态 Pause/Continue**：不同于 Envoy，插件拿到哪种 body 模式在模块加载时就由编译出的 guest 导出了哪些钩子决定，而不是由 guest 每次请求返回的 `Action` 决定。见[两种 body 模式，静态选择](#两种-body-模式静态选择)。
- **无 gRPC callout**（`proxy_dispatch_grpc_call`）：调用即 trap。
- **响应/body 阶段不能 callout**（同步阶段），且请求阶段 dispatch 后不 Pause 会被丢弃。tick 阶段的 callout 使用所属插件的 `calloutAllowlist`（见[定时器](#shared-data-与定时器)）。
- **无 shared queue、foreign function、guest 自定义 metrics**：调用即 trap。
- **请求级上下文按钩子调用创建**：同一请求的请求头钩子与响应头钩子可能运行在不同池化实例上、拿到全新 context id。分别配置的阶段通过显式请求头关联；这些条目不共享 guest 内存或 shared data。
- **`source.oci` v1 限制**：镜像仓库必须提供可公开验证的 TLS 证书（本地测试用的 loopback 仓库可使用明文 HTTP）；不支持镜像仓库镜像（registry mirror）。见[`source.oci`——从 OCI 镜像仓库拉取](#sourceoci从-oci-镜像仓库拉取)。

## 编写 guest

任何有 proxy-wasm SDK 的语言都可以，只要编译产物面向线性内存 Wasm 目标（无需 WASI 导入——Rust SDK 用 `wasm32-unknown-unknown` 即可）。本仓库测试用的参考 guest 是很好的起点：

- 源码：`edgion-tests/wasm/header-demo/src/lib.rs`
- 使用官方 [`proxy-wasm` Rust SDK](https://crates.io/crates/proxy-wasm) v0.2 构建。
- `Cargo.toml`：`crate-type = ["cdylib"]`、`proxy-wasm = "0.2"`、自带 `[workspace]` 表（不进主 Edgion workspace，因为它只面向 `wasm32-unknown-unknown` 构建）。
- 构建：`rustup target add wasm32-unknown-unknown`，然后 `./edgion-tests/wasm/build.sh`——release 编译并把产物拷到 `edgion-tests/wasm/artifacts/header_demo.wasm`。

guest 的最小形态（摘自参考模块）：

```rust
use proxy_wasm::traits::{Context, HttpContext};
use proxy_wasm::types::Action;

proxy_wasm::main! {{
    proxy_wasm::set_http_context(|_context_id, _root_context_id| -> Box<dyn HttpContext> {
        Box::new(MyPlugin)
    });
}}

struct MyPlugin;
impl Context for MyPlugin {}

impl HttpContext for MyPlugin {
    fn on_http_request_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.set_http_request_header("x-wasm-added", Some("yes"));
        Action::Continue
    }

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.set_http_response_header("x-wasm-resp", Some("yes"));
        Action::Continue
    }
}
```

外部鉴权 callout guest 形如：

```rust
impl HttpContext for AuthPlugin {
    fn on_http_request_headers(&mut self, _n: usize, _eos: bool) -> Action {
        self.dispatch_http_call(
            "https://auth.internal:8443",       // origin — must be in calloutAllowlist
            vec![(":method", "GET"), (":path", "/verify"), (":authority", "auth.internal")],
            None, vec![], Duration::from_millis(500),
        ).unwrap();
        Action::Pause
    }
}

impl Context for AuthPlugin {
    fn on_http_call_response(&mut self, _token: u32, num_headers: usize, _body: usize, _trailers: usize) {
        if num_headers > 0 && self.get_http_call_response_header(":status").as_deref() == Some("200") {
            self.resume_http_request();
        } else {
            self.send_http_response(403, vec![], Some(b"denied"));
        }
    }
}
```

参考 guest 实现的完整契约（含 panic、本地响应测试路径）与产物重建方法见 `edgion-tests/wasm/README.md`。参考 guest 同时演练了完整请求 body 模式（`on_http_request_body`）和按 chunk 的响应 body 观测（`on_http_response_body`）。

## FAQ

### Q：支持哪些 proxy-wasm ABI 版本？

`0.2.0` 与 `0.2.1`（Rust/C++ SDK 导出的 `proxy_abi_version_0_2_x`）。不支持或混合的标记集合会使兼容性检查失败；完全没有标记时，只要 import 与 hook 均兼容，模块会在 WARN 后加载。

### Q：我的 callout 从未被执行？

依次检查：（1）目标 origin 是否列在 `calloutAllowlist`——空列表完全禁用 callout；（2）请求头阶段 dispatch 后 guest 是否返回了 `Action::Pause`——dispatch 后 `Continue` 会被丢弃（访问日志出现 `callouts-dropped=N`）；（3）dispatch 是否发生在请求头或 tick 阶段——请求 body、响应头、响应 body 阶段均返回 `UNIMPLEMENTED`；（4）tick 阶段必须由所属插件自身授权目标地址，其他条目的 allowlist 不适用。

### Q：我的请求意外返回了 `500`，而不是运行 `proxy_on_request_body`？

确认 `Wasm` 条目位于 `requestPlugins`，并且编译出的 guest 确实导出了 `proxy_on_request_body`。然后检查网关物理上限和请求是否允许缓冲（gRPC 和携带 body 的升级请求会被拒绝）。如果条目配置了 `body:`，还应检查其 `conditions`、`maxBodySize` 和 `onReadFailure`：fail-close 的限制或读取失败会拒绝请求，fail-open 的策略失败会跳过整个条目；准入拒绝和网关物理上限绝不会 fail-open。

### Q：guest 模块在响应阶段不生效？

确认条目列在 `upstreamResponseFilterPlugins`（响应头）或 `upstreamResponseBodyFilterPlugins`（只读 body）下，而不是 `upstreamResponsePlugins`。ExtProc 只在 `requestPlugins` 中配置一次；响应处理由该请求条目持有的内部 companion 执行，并不是可单独配置的异步阶段条目。

### Q：controller 拒绝了我的 EdgionPlugins 资源？

常见原因：`source` 的 `file`/`inline`/`url`/`oci` 设置了零个或多个；`source.url` 未配 `sha256`；`sha256` 不是 64 位十六进制；`source.inline` 不是合法 base64 或解码超过 1MiB；`source.fetch` 未配 `source.url`；`source.oci.image` 不是合法的 `registry/repo:tag` 或 `registry/repo@sha256:<digest>` 引用；`calloutAllowlist` 条目不是干净的 `scheme://host[:port]` origin；`timeoutMs`/`calloutTimeoutMs` 超出 `1..=10000`；`instancePoolSize` 超出 `1..=64`。

### Q：我覆写了磁盘上的 `.wasm` 文件，但网关还在跑旧构建？

覆写文件后更新所属 `EdgionPlugins` 资源，触发插件实例重建。新运行时会重新加载文件；旧运行时随最后一个插件引用和在途租约结束而释放。单纯覆写不会替换已经加载的运行时。

### Q：两个请求之间能共享状态吗？

能——用 `proxy_{get,set}_shared_data`，同一个配置插件实例的所有池化 VM 在**单个网关进程内**共享。分别配置的条目或阶段不共享这些状态。它不跨网关副本复制；集群级协调仍属于 `rate_limit_redis`。

## 相关链接

- [ExtProc 插件](./ext-proc.md)——gRPC 推模式方案（见 [callout 与 ExtProc 的分工](#callout-与-extproc-的分工)）
- [Filters 总览](../README.md)
