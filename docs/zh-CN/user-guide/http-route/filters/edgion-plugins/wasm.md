# Wasm 插件

> **🔌 Edgion 扩展**
>
> Wasm 插件在网关进程内通过 [wasmtime](https://wasmtime.dev/) 直接执行 [proxy-wasm](https://github.com/proxy-wasm/spec) ABI guest 模块——一个用 Rust、C++、Go、TinyGo 或任意 proxy-wasm SDK 编译出的普通 WebAssembly 二进制文件。它由 `EdgionPlugins` CRD 提供，不属于标准 Gateway API。

## Wasm 解决什么问题？

proxy-wasm 是一套可移植的代理扩展 ABI：编写一次扩展，即可运行在任意兼容 host（Envoy、本网关或其他实现）上，无需重新编译 host 本身。你不需要针对 Edgion 内部 API 编写原生 Rust 插件，而是用你偏好语言的 [proxy-wasm SDK](https://crates.io/crates/proxy-wasm) 编译出一个体积较小的 WebAssembly 模块，把编译产物 `.wasm` 字节码交给本插件。网关只编译一次、缓存编译结果，并对初始化好的实例做池化，因此单次请求的执行成本很低。

实现的 ABI 是一个刻意收敛的子集（见下文）：header 读写、本地响应、**HTTP callout**（请求阶段 Pause/恢复，以及 `on_tick` 内发起）、**响应 body 只读观测**、**shared data** 与**定时器**（`on_tick`）。body *改写*与请求 body 钩子未实现。

## 支持的 ABI 子集

### 钩子（Hooks）

| proxy-wasm 钩子 | EdgionPlugins 阶段 | 运行时机 |
|---|---|---|
| `proxy_on_context_create` / `proxy_on_vm_start` / `proxy_on_configure` | —（实例初始化） | 每个池化实例在服务任何请求之前执行一次 |
| `proxy_on_request_headers` | `requestPlugins`（`RequestFilter`） | 请求 header 到达、访问后端之前 |
| `proxy_on_http_call_response` | —（callout 回调） | 每个完成的 callout 回调一次：请求阶段发起的在挂起的请求上下文上，`proxy_on_tick` 发起的在 root context 上 |
| `proxy_on_response_headers` | `upstreamResponseFilterPlugins`（`UpstreamResponseFilter`） | 上游响应 header 到达 |
| `proxy_on_response_body` | `upstreamResponseBodyFilterPlugins`（`UpstreamResponseBodyFilter`） | 每个上游 body chunk 一次，**只读**（见 [body 观测](#响应-body-观测只读)） |
| `proxy_on_tick` | —（后台） | guest 设定 tick 周期后周期性触发（见[定时器](#shared-data-与定时器)） |
| `proxy_on_done` / `proxy_on_delete` | —（上下文清理） | 每个请求级上下文结束之后 |

Wasm **不支持**（异步的）`upstreamResponsePlugins` 阶段。

### Host 调用

| Host 函数 | 支持 | 说明 |
|---|:---:|---|
| `proxy_get_header_map_pairs` / `proxy_get_header_map_value` | ✅ | 请求头、响应头，以及（callout 回调内）`HttpCallResponseHeaders`——callout 响应头外加合成的 `:status` |
| `proxy_replace_header_map_value` / `proxy_add_header_map_value` / `proxy_remove_header_map_value` / `proxy_set_header_map_pairs` | ✅ | 设置/追加/删除/整体替换；钩子返回后应用到真实请求/响应。对 callout 响应 map 的写入被拒绝 |
| `proxy_send_local_response` | ✅ | 以指定状态码/响应体/头短路请求。也可在 `proxy_on_http_call_response` 内调用（鉴权 callout 后拒绝） |
| `proxy_http_call` | ✅（请求 + tick 阶段） | 见 [HTTP callout](#http-calloutpause恢复)。响应/body 阶段返回 `UNIMPLEMENTED`（12）；tick 阶段要求 allowlist 无歧义（见[定时器](#shared-data-与定时器)） |
| `proxy_continue_request` / `proxy_continue_stream(Request)` | ✅ | 恢复因 callout 挂起的请求。`proxy_continue_response` 仍是接受型 no-op |
| `proxy_get_shared_data` / `proxy_set_shared_data` | ✅ | 模块级 KV + CAS——见 [shared data](#shared-data-与定时器) |
| `proxy_set_tick_period_milliseconds` | ✅ | 启动模块级 tick 任务；`0` 停止。周期下限 100ms |
| `proxy_get_property` / `proxy_set_property` | ✅（基础子集） | 可读键：`request.path`、`request.method`、`request.host`、`source.address`、`plugin_name`、`response.code`（仅响应阶段）。`proxy_set_property` 仅写入当前阶段的内存属性表 |
| `proxy_get_buffer_bytes` | ✅ | `VmConfiguration`、`PluginConfiguration`、`HttpCallResponseBody`（callout 回调内）、`HttpResponseBody`（body 钩子内的**当前 chunk 窗口**） |
| `proxy_log` | ✅ | 转发到网关自身的 `tracing::debug!` 输出（target `wasm_guest`），**绝不**写入访问日志 |
| `proxy_get_current_time_nanoseconds` | ✅ | 系统时钟 |
| `proxy_get_configuration`（0.1 遗留） | ✅ | 返回与 `PluginConfiguration` buffer 相同的字节 |
| `proxy_set_effective_context` / `proxy_done` | 接受型 no-op | 返回 `STATUS_OK`，不做任何事 |
| 其余全部（请求 body 钩子、gRPC callout、shared queue、foreign function、metrics 等） | ❌ | 未注册进 linker。guest 导入且真正调用会 **trap**（随后按[失败语义](#失败语义)处理）；只导入不调用无害 |

### ABI 版本检查

加载时 host 会检查 guest 导出的 `proxy_abi_version_*` 标记：

- `proxy_abi_version_0_2_0` / `proxy_abi_version_0_2_1`——接受。
- 其他 `proxy_abi_version_*`——**加载错误**，报错信息明确为 "unsupported proxy-wasm ABI version"（而不是令人困惑的运行时 trap）。
- 完全没有标记——WARN 后放行（兼容手写测试模块与非标 guest）。

## 配置参考

```yaml
type: Wasm
config:
  source:
    file: ./plugins/header_demo.wasm
  pluginConfig: '{"add_header":"x-wasm-added"}'
  failOpen: false
  timeoutMs: 100
```

| 参数 | 类型 | 默认值 | 说明 |
|-----------|------|---------|-------------|
| `source.file` | String | — | 本地 `.wasm` 文件路径，相对路径基于网关工作目录解析。`file` / `inline` / `url` 三者必须恰好设置一个。 |
| `source.inline` | String (base64) | — | base64 编码、内嵌在资源里的字节码。**仅作为开发/测试便利**——解码后硬上限 1MiB（超出为校验错误），超过 512KiB 网关在加载时 WARN。见[来源形态选型](#来源形态选型)。 |
| `source.url` | String | — | 网关拉取字节码的 `http://` / `https://` URL。**必须配 `sha256`。** 连接行为可通过 `source.fetch` 调整。 |
| `source.fetch` | Object | — | `url` 来源的可选拉取强化（与 `file`/`inline` 同用为校验错误）：TLS、认证、超时、重试。 |
| `source.fetch.tls` | Object | — | 共享 `TlsConfig` 结构：自定义 CA（`validation.caCertificateRefs`）和/或 mTLS 客户端身份（`clientCertificateRef`）。Secret 引用由 controller 解析（跨 namespace 需要 `ReferenceGrant`）。 |
| `source.fetch.authHeaderSecretRef` | Object | — | `{name, namespace?, key}`：Secret 的 `key`（默认 `authorization`）保存拉取时发送的完整 `Authorization` 头值（如 `Bearer <token>`）。设置了引用但 Secret 尚未解析时，加载**失败**而不是静默匿名拉取。 |
| `source.fetch.timeoutMs` | Integer | `10000` | 单次尝试超时。范围 `1..=60000`。 |
| `source.fetch.retry` | Object | — | `{attempts, backoffMs}`——含首次的总尝试数（`1..=10`，默认 3）与逐次翻倍的基础退避（默认 500，上限 30000）。策略固定：传输错误与 `5xx` 重试；**`4xx` 永不重试**。 |
| `sha256` | String (64 hex) | — | 原始字节码的 SHA-256，编译前校验。`file` / `inline` 可选；`url` **必填**。 |
| `pluginConfig` | String | — | 传给 `proxy_on_configure` 的不透明配置，经 `PluginConfiguration` buffer 可读。 |
| `vmConfig` | String | — | 传给 `proxy_on_vm_start` 的不透明配置，经 `VmConfiguration` buffer 可读。 |
| `failOpen` | Boolean | `false` | 加载失败 / trap / 超时 / callout 不恢复时的行为——见[失败语义](#失败语义)。 |
| `timeoutMs` | Integer | `100` | 每次钩子调用的墙钟预算（epoch 中断）。范围 `1..=10000`。body chunk 钩子按 `timeoutMs / 10` 执行。 |
| `instancePoolSize` | Integer | `8` | 每个编译模块、每个网关进程保留的池化实例上限。范围 `1..=64`。 |
| `calloutAllowlist` | String 列表 | `[]` | guest 可通过 `proxy_http_call` 访问的 origin（`scheme://host[:port]`）。**空 = 禁用 callout**（dispatch 返回 guest 可观测的宿主错误）。规范化后精确匹配。 |
| `calloutTimeoutMs` | Integer | `5000` | 每个 callout 的 guest 超时上限（guest 传 0 时的生效值）。范围 `1..=10000`。 |

## 来源形态选型

| 形态 | 定位 | 大小上限 | 分发路径 |
|---|---|---|---|
| `inline` | **开发/测试便利**，不是生产分发机制 | 解码后 1MiB（硬上限）；另受 K8s 对象/etcd 限制 | 随 CR 进 etcd；每次 conf_sync 全量携带 |
| `file` | 打进网关镜像或 initContainer 挂载的模块 | 无 | 镜像构建 / volume 挂载 |
| `url` | **生产推荐**——内部制品库（Nexus、Artifactory、S3 presigned） | 无 | 网关直拉，配合 `source.fetch` 的 TLS/认证/重试与强制 `sha256` |
| `oci://` | Roadmap（对齐 Istio 的 OCI 镜像分发）——尚未实现 | — | — |

这与 Istio 的取向一致：Istio 的 `WasmPlugin` **根本没有 inline 字段**，以 `oci://` 为主通道、`http(s)` 次之。Edgion 保留 inline 用于快速迭代，但加上限并告警；ConfigMap 形态经评估后否决（与 inline 同受 etcd 对象上限，还引入 ref 追踪复杂度，无增量价值）。

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
- **请求 + tick 阶段**：响应头/body 阶段是同步的、无法 await，dispatch 返回 `UNIMPLEMENTED`。`on_tick` 内的 callout 已支持，回调投递到 root context——额外的"allowlist 一致"条件见[定时器](#shared-data-与定时器)。
- **必须 Pause**（请求阶段）：`dispatch` + `Action::Continue` 不受支持——排队的 callout 被丢弃并记日志（`callouts-dropped=N`）。dispatch 之后必须 Pause。（`on_tick` 没有 Pause 概念：在那里 dispatch 的 callout 总是在钩子返回后执行。）
- **预算**：每请求（或每 tick）最多 8 次 dispatch、4 轮回调（回调里可以追发 callout）、全进程 64 个并发 callout、单 callout 响应体上限 256KiB。每个 callout 的超时为 `min(guest 超时, calloutTimeoutMs)`。
- **callout 失败**（连接错误、超时、响应体超限）：回调以 **0 个 header** 触发（proxy-wasm 惯例），guest 自行决定失败行为。宿主绝不因 callout 失败而 trap guest。
- **guest 不恢复**：所有 callout 完成后 guest 既没 continue 也没本地应答 → 宿主按 `failOpen` 处理（reason `no-resume`）。
- **epoch 预算**：`timeoutMs` 只约束每次 *guest* 钩子调用；callout 的网络等待发生在 guest 调用之间，不消耗 guest CPU 预算。

### callout 与 ExtProc 的分工

两者都是接入外部决策服务，方向不同：

- **ExtProc**（推模式）：网关把每个请求各阶段推给常驻外部处理器（gRPC）。外部服务拥有逻辑、需要看到全部流量时选它。
- **Wasm callout**（拉模式）：guest 逻辑内嵌网关，按需外呼。插件自己决定*是否/何时*咨询外部服务时（如先查缓存、未命中再校验 token）选它。

## 响应 body 观测（只读）

把 Wasm 条目列在 `upstreamResponseBodyFilterPlugins` 下，`proxy_on_response_body` 会对每个上游 chunk 触发一次。

> ### ⚠️ 这里偏离 proxy-wasm spec——移植 guest 前必读
>
> spec 的 body 契约是**累积式**的：`body_size` 是**宿主到目前为止已缓冲的总字节数**，`Action::Pause` 让宿主**继续缓冲**，`proxy_get_buffer_bytes` 读的是**累积缓冲区**。Envoy 的实现方式是把 Pause 映射成 `StopIterationAndBuffer`，由它的 filter manager 持有缓冲区。
>
> **本网关实现的是每 chunk 的只读*窗口***：`body_size` 是**当前 chunk** 的长度，`proxy_get_buffer_bytes(HttpResponseBody, …)` **只返回该 chunk**，body 从不被缓冲。
>
> **后果**：一个按标准范式写的 guest——
> ```rust
> if !end_of_stream { return Action::Pause; }             // 期望宿主缓冲
> let body = self.get_http_response_body(0, body_size)?;  // 以为拿到完整 body
> ```
> ——**只会看到最后一个 chunk**。对 WAF 或敏感数据扫描器来说这是**静默漏检**。因此宿主**不会**静默忽略 Pause：它会递增 `edgion_wasm_body_pause_unsupported_total{module}` 并按模块告警一次。**请对该指标配告警**——它意味着线上某个 guest 正在看不完整的数据。
>
> 完整 spec 语义需要请求/响应 body 缓冲基建，这是**有意不实现**的（代价是内存、破坏流式上传、TTFB 变差；而且在本代理内核上，请求 body 的*改写*还额外撞上"上游 Content-Length 早于 body filter 发出"的顺序问题）。**后果：现成的 proxy-wasm WAF / body 扫描类 guest——它们清一色用 Pause 攒 body 的范式——在这里跑不了。** 本网关的 body 观测按设计就是每 chunk 的，请照此编写 guest。

在每 chunk 模型下正确的用法：

- 跨 chunk 聚合是 **guest 自己的事**（用它自己的内存，或 [shared data](#shared-data-与定时器)——参考 guest 就是把每个 chunk 的字节数累加进一个 shared 计数器）。
- **严格只读**：不能改写 body，也不能在 body 阶段短路响应（响应头已发出）。检测结论只能记日志（`proxy_log`）或写入 shared data。
- 每个 chunk 在缩减的 CPU 预算（`timeoutMs / 10`，下限 10ms）下执行，慢 guest 拖不垮长流；body 钩子 trap **绝不断流**（实例被丢弃，流继续，与 `failOpen` 无关）。
- 未导出 `proxy_on_response_body` 的 guest 在此阶段是零成本 no-op。

请求 body 观测**完全未实现**（没有 `proxy_on_request_body`）——它依赖同一套 body 缓冲基建。

## Shared data 与定时器

### Shared data

`proxy_get_shared_data` / `proxy_set_shared_data` 提供带 proxy-wasm CAS 语义的模块级 KV（每 key 单调递增的 CAS 令牌；`set` 携带非零期望 CAS 且不匹配时返回 `CasMismatch`；CAS 0 无条件写；空值删除 key）。

- **作用域：单个网关进程内的一个模块（缓存条目）。** 该模块的所有池化实例共享——跨请求、跨阶段、含 tick 任务。它**不是**集群级状态：集群级协调（如全局限流）仍然用 `rate_limit_redis`，不要用 shared data。
- **信任含义**：所有解析到同一份字节码的 `EdgionPlugins` 条目共享这个 store——包括 tick 阶段按某一个插件的 allowlist 外呼拉回的数据，流经*其他*插件的请求也读得到。数据必须隔离的信任域之间不要共享同一模块；按租户分发不同的字节码（或不同来源）。
- 上限：单值 64KiB，每模块总量 4MiB。超限写入返回 `BAD_ARGUMENT`。
- 生命周期：模块缓存条目被淘汰时释放（见[模块缓存生命周期](#模块缓存生命周期)）；换内容的 reload 从空 store 开始。

### 定时器（`on_tick`）

guest 调用 `proxy_set_tick_period_milliseconds(N)`（通常在 `on_configure` 里）后，后台任务（每模块一个）每 `N` ms（下限 100ms）调用一次 `proxy_on_tick`。tick 实例从与请求相同的池租借，受正常 `timeoutMs` epoch 预算约束，且**相邻两次 tick 可能落在不同的池化实例上**——root context 的结构体字段是实例本地的，需要跨 tick 存活的状态请放 shared data（在 `on_configure` 里*初始化*的状态没问题，比如解析好的配置：每个池化实例初始化时都会跑 `on_configure`）。guest 可在 `on_tick` 内调整周期或停止（`0`）。模块缓存条目被淘汰时 tick 任务自动停止。

典型模式：`on_tick` 发 callout 拉取最新规则集写进 shared data，请求钩子读取（限流规则热刷新、风控名单同步、配置轮询）。

#### `on_tick` 内的 callout

`on_tick` 可以像请求阶段一样 dispatch HTTP callout，差异在于：

- **没有 Pause/恢复**：`proxy_on_tick` 没有 Action 返回值；钩子期间排队的 callout 在钩子返回后立即执行，每个结果经 `proxy_on_http_call_response` 投递到 **root context**（与实例同生命周期——不涉及任何请求级上下文）。
- **要求 allowlist 一致。** tick 任务是*模块级*的，而 `calloutAllowlist` 是*每插件*的——多个 `EdgionPlugins` 条目指向同一份字节码时共享同一个模块（和同一个 tick 任务）。因此 tick 阶段只有在引用该模块的插件对**唯一**一份非空 allowlist 达成一致时才启用 dispatch：
  - `calloutAllowlist` 为**空**的条目（例如同一模块还挂在响应/body 阶段但没配 callout）视为"无主张"，不会阻断 tick callout。
  - 同一模块出现 ≥2 份**不同**的非空 allowlist 即**冲突**：tick dispatch 保持禁用（`UNIMPLEMENTED`，与该特性之前的行为一致），记一条 WARN，并且每个 tick 递增 `edgion_wasm_callout_total{result="tick_config_conflict"}`。这是有意的 fail-safe——否则一个插件的 allowlist 会泄漏到另一个插件的 tick 执行里。
  - 冲突方被移除或 allowlist 对齐后自动恢复。
- 生效的超时上限是授权插件中 `calloutTimeoutMs` 的**最小值**；其余 callout 护栏（每 tick 8 次 dispatch、4 轮、并发上限、响应体上限）不变。

## 失败语义

所有失败模式共用同一个 `failOpen` 决策：

| Reason | 触发条件 | PluginLog |
|---|---|---|
| `load` | 模块下载/解码/编译失败、`sha256` 不匹配、ABI 版本不支持、或实例初始化失败（`on_vm_start`/`on_configure` 返回 false）。加载错误缓存 30s。 | `wasm load:fail-open` / `fail-close` |
| `acquire` | 请求时实例池无法产出可用 VM（如 pooling allocator 槽位耗尽）。 | `wasm acquire:…` |
| `trap` | guest panic、`unreachable`、CPU 预算超限，或在钩子（含 callout 回调与上下文关闭）中出错。 | `wasm trap:…` |
| `callout` | callout 解析结构性失败（如 guest 回调在投递中 trap）。 | `wasm callout:…` |
| `no-resume` | guest 因 callout 挂起，但全部完成后既未恢复也未应答。 | `wasm no-resume:…` |
| `not loaded` | 仅响应阶段：同步钩子用非阻塞缓存查询；模块（异步的）首次加载未完成时触发，同时踢一次后台加载。 | `wasm not loaded:…` |

- **`failOpen: false`**（默认）——请求以 `500` + `wasm plugin error` 短路。
- **`failOpen: true`**——记录失败，插件链继续。

### trap 污染与恢复

trap（或其他故障）的 VM 实例会被**污染**：直接丢弃而不回池，释放其槽位。下一个请求从缓存的编译模块重新实例化（便宜——只有 `Module::new` 昂贵）。guest panic 绝不会 crash、挂起或损害网关进程。

### 超时

每次钩子调用由 `timeoutMs` 经 wasmtime **epoch 中断**约束：后台 ticker 每 10ms 递增引擎 epoch，deadline 在*每次* guest 调用（header 钩子、callout 回调、body chunk、tick）开始时重新武装。有效粒度约 10-20ms。callout 的网络等待发生在 guest 调用之间，不占 guest 预算。

## 模块缓存生命周期

编译模块缓存的 key 是来源本身，**外加 `file` 来源的文件 size+mtime**：

- **引用计数淘汰。** 每个存活的 Wasm 插件实例都会 pin 住其缓存条目。reload/配置变更使某条目失去最后一个引用（如 inline 内容变化 → 新 key）后，孤儿条目在 **60s 宽限期**后被淘汰（宽限吸收 reload 抖动）。在途请求通过自身强引用保持条目存活；最后一个结束后内存释放。淘汰计入 `edgion_wasm_module_evicted_total` 指标。
- **`file` 来源内容感知。** 运行时缓存 key 含文件 size 与 mtime，在插件（重）构建时采集。同路径覆写 `.wasm` **并触发插件 reload**（对所属 `EdgionPlugins` 资源的任何更新，或全量配置重同步）即可生效新内容；旧条目被引用计数淘汰。不 reload 的话运行中插件仍持旧 key——单纯覆写文件仍不会热替换。
- **实例预热。** 加载时即实例化一个实例：guest 初始化失败成为清晰的加载错误（进负缓存），而不是每请求的 acquire 失败；`on_configure` 里设置的 tick 周期在首个请求到来前就开始生效。
- **多租户容量。** 所有 Wasm 模块共享一个引擎级 pooling allocator。`instancePoolSize` 要按全部已配置 Wasm 插件的实例总量核算——峰值实例数约为 `edgion_wasm_modules_loaded × instancePoolSize`。

## 指标

所有指标带 `module` label——人类可读的来源描述（`file:<path>`、`url:<url>`、`inline:<sha8>`），基数受配置模块数约束。

| 指标 | 类型 | Labels | 含义 |
|---|---|---|---|
| `edgion_wasm_module_load_total` | counter | `result=ok\|error` | 加载尝试（负缓存命中不计） |
| `edgion_wasm_module_compile_duration_ms` | histogram | | `Module::new` 耗时 |
| `edgion_wasm_module_evicted_total` | counter | | 引用计数淘汰次数 |
| `edgion_wasm_instance_instantiate_total` | counter | | 新实例化（池 miss） |
| `edgion_wasm_instance_discard_total` | counter | `reason=poisoned\|over_capacity` | 未回池而被丢弃的实例 |
| `edgion_wasm_pool_size` | gauge | | 当前池内实例数 |
| `edgion_wasm_hook_duration_ms` | histogram | `phase` | guest 钩子耗时（`request_headers`、`response_headers`、`response_body`、`http_call_response`、`tick`） |
| `edgion_wasm_trap_total` | counter | `phase` | trap / 协议违规 |
| `edgion_wasm_fail_mode_total` | counter | `reason`、`mode=open\|close` | fail-mode 决策 |
| `edgion_wasm_callout_total` | counter | `result=ok\|error\|denied\|over_limit\|tick_config_conflict` | callout 结果；`tick_config_conflict` 统计因共享模块的插件配置了不同 `calloutAllowlist` 而保持禁用 dispatch 的 tick 次数 |
| `edgion_wasm_modules_loaded` | gauge | | 全局缓存中当前的编译模块数 |
| `edgion_wasm_shared_data_bytes` | gauge | `module` | 每个模块当前 shared-data 字节总量（上限 4MiB） |
| `edgion_wasm_body_pause_unsupported_total` | counter | `module` | **建议配告警。** guest 在 body 钩子返回了 `Action::Pause`，即它期望 spec 的累积缓冲语义（本 host 未实现）——它正在静默地只看到当前 chunk，而非完整 body |

trap 风暴表现为 `edgion_wasm_trap_total` 与 `edgion_wasm_instance_discard_total{reason="poisoned"}` 同步上涨。

## `PluginLog` 格式

- `wasm ok ops=N logs=M`——成功；应用了 `N` 个 header 操作，guest 输出 `M` 行日志（内容进 debug 日志）。
- `wasm ok ops=N pause-ignored`——guest 返回 `Pause` 但既没 dispatch callout 也没发本地响应；请求继续。
- `wasm callouts=N leftover=M`——请求阶段解析了 `N` 个 callout（`M` 个被轮次上限丢弃）。
- `wasm callouts-dropped=N`——guest dispatch 了 callout 却返回 `Continue`，被丢弃（不支持的用法）。
- `wasm local <status>`——guest 发送了本地响应。
- `wasm <reason>:fail-open` / `wasm <reason>:fail-close`——上文失败原因之一。

## 剩余限制

- **无请求 body 钩子**（`proxy_on_request_body`）：依赖网关尚无的请求 body 缓冲；只实现请求 body 钩子的 guest 永远不会被调用。
- **响应 body 仅可观测，且是每 chunk 语义（非 spec 的累积语义）**：不能改写 body，不能在 body 阶段短路，`Action::Pause` 无法兑现——按标准缓冲范式写的 guest 只会看到当前 chunk。见[响应 body 观测](#响应-body-观测只读)一节的警告。
- **无 gRPC callout**（`proxy_dispatch_grpc_call`）：调用即 trap。
- **响应/body 阶段不能 callout**（同步阶段），且请求阶段 dispatch 后不 Pause 会被丢弃。tick 阶段的 callout 还额外要求共享模块的插件对 `calloutAllowlist` 达成一致（见[定时器](#shared-data-与定时器)）。
- **无 shared queue、foreign function、guest 自定义 metrics**：调用即 trap。
- **请求级上下文按钩子调用创建**：同一请求的请求头钩子与响应头钩子可能运行在不同池化实例上、拿到全新 context id。跨阶段关联请用 shared data 或塞请求头，不要依赖 guest 结构体字段。
- **暂无 OCI（`oci://`）来源**——roadmap。

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

参考 guest 实现的完整契约（含 panic、本地响应测试路径）与产物重建方法见 `edgion-tests/wasm/README.md`。

## FAQ

### Q：支持哪些 proxy-wasm ABI 版本？

`0.2.0` 与 `0.2.1`（Rust/C++ SDK 导出的 `proxy_abi_version_0_2_x`）。导出其他 `proxy_abi_version_*` 标记的模块在加载时以明确信息失败；不导出任何标记的模块 WARN 后放行。

### Q：我的 callout 从未被执行？

依次检查：（1）目标 origin 是否列在 `calloutAllowlist`——空列表完全禁用 callout；（2）请求阶段 dispatch 后 guest 是否返回了 `Action::Pause`——dispatch 后 `Continue` 会被丢弃（访问日志出现 `callouts-dropped=N`）；（3）dispatch 是否发生在请求或 tick 阶段——响应/body 阶段返回 `UNIMPLEMENTED`；（4）tick 阶段 dispatch 还要求无 allowlist 冲突——多个插件以不同 `calloutAllowlist` 共享同一模块时，查看 `edgion_wasm_callout_total{result="tick_config_conflict"}` 与 WARN 日志。

### Q：guest 模块在响应阶段不生效？

确认条目列在 `upstreamResponseFilterPlugins`（响应头）或 `upstreamResponseBodyFilterPlugins`（只读 body）下，而不是 `upstreamResponsePlugins`（那个异步阶段属于 ExtProc 等其他插件）。

### Q：controller 拒绝了我的 EdgionPlugins 资源？

常见原因：`source` 的 `file`/`inline`/`url` 设置了零个或多个；`source.url` 未配 `sha256`；`sha256` 不是 64 位十六进制；`source.inline` 不是合法 base64 或解码超过 1MiB；`source.fetch` 未配 `source.url`；`calloutAllowlist` 条目不是干净的 `scheme://host[:port]` origin；`timeoutMs`/`calloutTimeoutMs` 超出 `1..=10000`；`instancePoolSize` 超出 `1..=64`。

### Q：我覆写了磁盘上的 `.wasm` 文件，但网关还在跑旧构建？

内容感知的缓存 key 在插件构建时采集。覆写文件后更新一次所属 `EdgionPlugins` 资源（任意变更）——reload 会重新 stat 文件、落到新缓存条目，旧条目在宽限期后被淘汰。

### Q：两个请求之间能共享状态吗？

能——用 `proxy_{get,set}_shared_data`，同一模块的所有实例在**单个网关进程内**共享。它不跨网关副本复制；集群级协调仍属于 `rate_limit_redis`。

## 相关链接

- [ExtProc 插件](./ext-proc.md)——gRPC 推模式方案（见 [callout 与 ExtProc 的分工](#callout-与-extproc-的分工)）
- [Filters 总览](../README.md)
