# Wasm 插件

> **🔌 Edgion 扩展**
>
> Wasm 插件在网关进程内通过 [wasmtime](https://wasmtime.dev/) 直接执行 [proxy-wasm](https://github.com/proxy-wasm/spec) ABI guest 模块——一个用 Rust、C++、Go、TinyGo 或任意 proxy-wasm SDK 编译出的普通 WebAssembly 二进制文件。它由 `EdgionPlugins` CRD 提供，不属于标准 Gateway API。

## Wasm 解决什么问题？

proxy-wasm 是一套可移植的代理扩展 ABI：编写一次扩展，即可运行在任意兼容 host（Envoy、本网关或其他实现）上，无需重新编译 host 本身。你不需要针对 Edgion 内部 API 编写原生 Rust 插件，而是用你偏好语言的 [proxy-wasm SDK](https://crates.io/crates/proxy-wasm) 编译出一个体积较小的 WebAssembly 模块，把编译产物 `.wasm` 字节码交给本插件。网关只编译一次、缓存编译结果，并对初始化好的实例做池化，因此单次请求的执行成本很低。

这是一个 **v1、仅 header** 的实现：只覆盖 proxy-wasm ABI 中经过刻意裁剪的一个子集（见下文），不是完整规范。

## 支持的 ABI 子集（v1）

### 钩子（Hooks）

| proxy-wasm 钩子 | EdgionPlugins 阶段 | 运行时机 |
|---|---|---|
| `proxy_on_context_create` / `proxy_on_vm_start` / `proxy_on_configure` | —（实例初始化） | 每个池化实例在服务任何请求之前执行一次 |
| `proxy_on_request_headers` | `requestPlugins`（`RequestFilter`） | 请求 header 到达、访问后端之前 |
| `proxy_on_response_headers` | `upstreamResponseFilterPlugins`（`UpstreamResponseFilter`） | 上游响应 header 到达 |
| `proxy_on_done` / `proxy_on_delete` | —（每次钩子调用后的清理） | 每次 header 钩子返回之后 |

Wasm **不支持** `upstreamResponseBodyFilterPlugins` 或（异步的）`upstreamResponsePlugins` 阶段——见[限制](#v1-限制)。

### Host 调用

| Host 函数 | 支持 | 说明 |
|---|:---:|---|
| `proxy_get_header_map_pairs` / `proxy_get_header_map_value` | ✅ | 请求与响应 header map |
| `proxy_replace_header_map_value` / `proxy_add_header_map_value` / `proxy_remove_header_map_value` / `proxy_set_header_map_pairs` | ✅ | 设置 / 追加 / 删除 / 批量替换；钩子返回后应用到真实的请求/响应上 |
| `proxy_send_local_response` | ✅ | 以指定状态码、body、header 短路请求 |
| `proxy_get_property` / `proxy_set_property` | ✅（基础子集） | 可读 key：`request.path`、`request.method`、`request.host`、`source.address`、`plugin_name`、`response.code`（仅响应阶段）。`proxy_set_property` 只写入当前阶段的内存态属性表（不持久化，不对外暴露） |
| `proxy_get_buffer_bytes` | ✅（仅 VM/插件配置） | 只支持 `VmConfiguration` 与 `PluginConfiguration` 两种 buffer 类型；其他类型返回 not-found |
| `proxy_log` | ✅ | 转发到网关自身的 `tracing::debug!` 输出（target 为 `wasm_guest`）——**绝不**写入 access log。access log 只记录一个汇总计数（见 [PluginLog 格式](#pluginlog-格式)） |
| `proxy_get_current_time_nanoseconds` | ✅ | 墙钟时间 |
| `proxy_get_configuration`（旧版 0.1） | ✅ | 返回与 `PluginConfiguration` buffer 相同的字节，兼容仍调用旧接口的 SDK |
| `proxy_set_effective_context` / `proxy_set_tick_period_milliseconds` / `proxy_done` / `proxy_continue_stream` / `proxy_continue_request` / `proxy_continue_response` | 接受但空操作 | 返回 `STATUS_OK` 但不做任何事（tick period 会被接受，但永远不会触发——v1 没有定时器子系统） |
| 其余全部（body 钩子、HTTP/gRPC callout、共享数据、共享队列、外部函数调用、指标等） | ❌ | 未注册到 linker。若 guest 模块导入了其中之一且**真的调用**它，该调用会 trap（后续按[失败语义](#失败语义)处理）；只导入但从不调用是无害的 |

## 配置参数

```yaml
type: Wasm
config:
  source:
    file: ./plugins/header_demo.wasm
  pluginConfig: '{"add_header":"x-wasm-added"}'
  failOpen: false
  timeoutMs: 100
```

| 参数 | 类型 | 默认 | 说明 |
|-----------|------|---------|-------------|
| `source.file` | String | — | 本地 `.wasm` 文件路径。相对路径相对于网关工作目录解析。`source.file` / `source.inline` / `source.url` 三者必须恰好设置一个。 |
| `source.inline` | String（base64） | — | 内联在资源中的 Wasm 字节码，base64 编码。 |
| `source.url` | String | — | 网关拉取字节码的 `http://` 或 `https://` URL（固定 10 秒拉取超时，只接受 `2xx` 响应）。**要求同时设置 `sha256`。** |
| `sha256` | String（64 位十六进制） | — | 原始字节码的 SHA-256，编译前校验。`file` / `inline` 可选；`url` **必填**。校验不通过即为加载失败——模块永远不会被编译。 |
| `pluginConfig` | String | — | 传给 guest `proxy_on_configure` 钩子的不透明配置字符串，通过 `PluginConfiguration` buffer（`proxy_get_buffer_bytes` / 旧版 `proxy_get_configuration`）读取。 |
| `vmConfig` | String | — | 传给 guest `proxy_on_vm_start` 钩子的不透明配置字符串，通过 `VmConfiguration` buffer 读取。 |
| `failOpen` | Boolean | `false` | 决定加载失败 / trap / 超时 / 实例获取失败时的行为——见[失败语义](#失败语义)。 |
| `timeoutMs` | Integer | `100` | 单次钩子调用的墙钟预算（毫秒），由 wasmtime epoch interruption 强制。范围 `1..=10000`。 |
| `instancePoolSize` | Integer | `8` | 每个编译模块、每个网关进程保留的最大池化实例数。范围 `1..=64`。 |

## 三种 source 形式

### `file` —— 本地文件系统

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-file
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      config:
        source:
          file: ./plugins/header_demo.wasm
        failOpen: false
        timeoutMs: 100
  upstreamResponseFilterPlugins:
    - type: Wasm
      config:
        source:
          file: ./plugins/header_demo.wasm
        failOpen: false
        timeoutMs: 100
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: wasm-route
  namespace: edgion-default
spec:
  parentRefs:
    - name: edgion-plugins-gateway
      namespace: edgion-test
  hostnames:
    - "wasm.example.com"
  rules:
    - matches:
        - path: { type: PathPrefix, value: "/" }
      filters:
        - type: ExtensionRef
          extensionRef: { group: edgion.io, kind: EdgionPlugins, name: wasm-file }
      backendRefs:
        - name: test-http
          namespace: edgion-test
          port: 30001
```

> 请求阶段和响应阶段是**两个独立条目**（`requestPlugins` 与 `upstreamResponseFilterPlugins`），和其他拆分阶段的插件一样。只需要哪个阶段就配哪个——缺省某个条目不会有额外开销。

### `inline` —— 字节码内联在资源里

适合体积较小、希望随资源一起分发、不依赖外部文件或网络的 guest：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-inline
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      config:
        source:
          inline: AGFzbQEAAAA...   # base64 编码的 .wasm 字节码
        failOpen: true
        timeoutMs: 50
```

两个内容不同的 inline source 不会在模块缓存中冲突——缓存 key 对完整 base64 内容做 SHA-256 哈希，而不是仅按长度区分。

### `url` —— 通过 HTTP(S) 拉取

这里 `sha256` 是**必填项**，以便网关在编译执行前校验拉取到的字节：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-url
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      config:
        source:
          url: "https://artifacts.example.com/plugins/header_demo.wasm"
        sha256: "3b1c...e4"   # 64 位十六进制，url source 必填
        failOpen: false
        timeoutMs: 200
        instancePoolSize: 16
```

## 失败语义

四种不同的失败原因都走同一个 `failOpen` 决策：

| 原因 | 触发条件 | PluginLog |
|---|---|---|
| `load` | 模块编译、下载、解码失败，或 `sha256` 校验不通过。加载错误会缓存 30 秒，避免对一个坏掉的 source 每次请求都重试；超过该窗口后会重新尝试。 | `wasm load:fail-open` / `wasm load:fail-close` |
| `acquire` | 实例池无法产出可用 VM（例如实例化过程中 `proxy_on_vm_start` / `proxy_on_configure` 返回 false）。 | `wasm acquire:fail-open` / `wasm acquire:fail-close` |
| `trap` | guest 在运行钩子期间 panic、命中 `unreachable`，或以其他方式出错。 | `wasm trap:fail-open` / `wasm trap:fail-close` |
| `not loaded` | 仅响应阶段：`UpstreamResponseFilter` 是同步钩子（无 `await`），因此使用非阻塞的缓存查找。若模块尚未完成其（异步的）首次加载，会立即触发该原因，并在后台为下次调用启动一次加载。 | `wasm not loaded:fail-open` / `wasm not loaded:fail-close` |

- **`failOpen: false`**（默认）——以 `500` 短路请求，body 为 `wasm plugin error`。
- **`failOpen: true`**——记录该次失败，插件链按未配置 Wasm 一样继续执行。

### Trap 中毒（poisoning）与恢复

一个 trap（或以其他方式出错）的 VM 实例会被**标记中毒**：它不会被归还实例池，而是直接丢弃，释放其占用的 slot。下一个需要该模块的请求会从缓存的编译模块重新实例化一个全新实例（不会重新编译——只有 `Module::new` 开销较大，`InstancePre::instantiate` 很廉价）。guest panic 永远不会导致网关进程崩溃、挂起或受损——只会导致那一次请求失败（受 `failOpen` 约束）。

### 超时

每次钩子调用都通过 wasmtime 的 **epoch interruption** 受 `timeoutMs` 约束：后台 ticker 每 10ms 递增一次引擎 epoch，钩子调用的截止时间在**每次**钩子调用开始时重新设置（而不是在实例被租出时只设置一次）——一次租用通常会横跨请求 header 钩子和响应 header 钩子，中间还有一次上游往返，如果只在租用时设置截止时间，等响应钩子运行时往往早已过期。超出预算的 guest 会 trap，处理方式与其他 trap 完全相同（中毒 + `failOpen` 决策）。

## 模块缓存生命周期

编译模块缓存（按 `source.cache_key()` 分 key）与网关进程同生命周期，请留意它「做」和「不做」的事：

- **配置变更不会淘汰缓存。** 一个编译后的模块会一直留在内存里，直到进程退出。修改 `EdgionPlugins` 资源、让 `source.inline` 或 `source.url` 携带不同的字节码内容，会生成一个**新的**缓存条目（内容哈希不同），而不会替换或淘汰旧的那一份——旧条目占用的内存只有在网关重启后才会被回收。
- **`file` source 按路径缓存，不是按内容。** 在磁盘上原地覆盖同一路径的 `.wasm` 文件**不会**被网关感知——一旦网关为某个路径编译过一次，就不会再重新读取该 `file` source。要发布新版本，请把 `source.file` 指向一个新路径（例如给文件名加版本号），或者重启网关。同理，如果 `source.url` 不变、只改了 `sha256`，也不会生效：缓存 key 由 source 本身派生，不含 `sha256`，因此一个不变的 `url` 会复用旧的编译产物，并不会用新的哈希重新校验。
- **多租户容量规划。** 所有 `EdgionPlugins` 资源里的 Wasm 模块共享同一个引擎级别的 pooling allocator。当其实例 slot 耗尽时，`instantiate` 会失败（按普通的 `acquire` 失败处理，受 `failOpen` 约束）。规划 `instancePoolSize` 时要考虑所有已配置 Wasm 插件加总后的实例数，而不是只看单个插件。
- **超时精度下限。** 超时的强制执行基于 epoch，ticker 每 10ms 触发一次，因此无论 `timeoutMs` 配成多少，实际生效粒度大约是 10-20ms。把 `timeoutMs` 设到 10 以下，guest 仍然能拿到大约一个 tick 的 CPU 时间才会 trap——它不是一个严格的亚 10ms 上限。

## PluginLog 格式

access log 会记录以下之一：

- `wasm ok ops=N logs=M` —— 成功；`N` 为应用到请求/响应上的 header 操作数，`M` 为通过 `proxy_log` 产生的日志行数（内容进 debug log，不进这里）。
- `wasm ok ops=N pause-ignored` —— guest 返回了 `Action::Pause` 但没有调用 `proxy_send_local_response`；v1 无法支持单纯的 pause（见[限制](#v1-限制)），请求/响应按正常流程继续。
- `wasm local <status>` —— guest 调用了 `proxy_send_local_response`；网关应用了该状态码/header/body 并终止处理。
- `wasm <reason>:fail-open` / `wasm <reason>:fail-close` —— 上述四种失败原因之一。

## v1 限制

- **仅 header，无 body 钩子。** 没有 `proxy_on_request_body` / `proxy_on_response_body` 等价物；只实现了 body 钩子的 guest 永远不会被调用。
- **不支持 HTTP 或 gRPC callout。** `proxy_dispatch_http_call` 等函数未注册——调用会 trap。
- **不支持定时器。** `proxy_set_tick_period_milliseconds` 会被接受（空操作）但永远不会触发；没有 `proxy_on_tick`。
- **不支持共享数据或共享队列。** proxy-wasm 的 shared-KV 与 shared-queue ABI 未实现；调用会 trap。
- **不支持 `Action::Pause`。** 一个钩子返回 `Pause` 但没有同时调用 `proxy_send_local_response`，会直接继续（日志记为 `pause-ignored`）——v1 没有中途挂起请求、稍后恢复的机制。
- **无跨阶段 guest 状态。** 同一个逻辑请求的请求-header 钩子和响应-header 钩子可能运行在**不同的池化 VM 实例**上（每个阶段各自从池中获取一次租用），且每次钩子调用都会拿到一个全新的 proxy-wasm context id。guest 在 `on_http_request_headers` 期间写入自身结构体字段的内容，不保证在 `on_http_response_headers` 期间可见。静态配置请使用 `pluginConfig` / `vmConfig`；若需要在两阶段之间关联信息，可以借助网关自身的 header 透传（例如把值暂存进一个请求 header）。

以上都不是永久性的架构限制——它们描述的是 v1 覆盖了什么，而不是 proxy-wasm 允许什么。

## 编写一个 guest

任何拥有 proxy-wasm SDK 的语言都可以使用，只要编译产物是线性内存的 Wasm target（不需要 WASI 导入——对 Rust SDK 而言 `wasm32-unknown-unknown` 已经足够）。本仓库自身测试所用的参考 guest 是一个很好的起点：

- 源码：`edgion-tests/wasm/header-demo/src/lib.rs`
- 使用官方 [`proxy-wasm` Rust SDK](https://crates.io/crates/proxy-wasm) v0.2 构建。
- `Cargo.toml`：`crate-type = ["cdylib"]`、`proxy-wasm = "0.2"`，并带有自己的 `[workspace]` 表（因此不参与主 Edgion Cargo workspace——它只为 `wasm32-unknown-unknown` 构建）。
- 构建：先 `rustup target add wasm32-unknown-unknown`，再执行 `./edgion-tests/wasm/build.sh`——以 release 模式编译，并把产物拷贝到 `edgion-tests/wasm/artifacts/header_demo.wasm`。

一个 guest 的最小形态（取自参考模块）：

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

完整契约（包括 panic 与 local-response 两条测试路径）参见 `edgion-tests/wasm/README.md`；修改 guest 源码后如何重新生成已提交的产物也写在那里。

## 常见问题

### Q：我的 guest 模块在响应阶段没有生效？

确认条目配置在 `upstreamResponseFilterPlugins` 下，而不是 `upstreamResponsePlugins`（后者属于另一个异步阶段，被 ExtProc 等插件使用）——Wasm 在响应侧只支持同步的 `UpstreamResponseFilter` 阶段。

### Q：Controller 拒绝了我的 EdgionPlugins 资源？

常见原因：`source` 里 `file`/`inline`/`url` 一个都没设置，或设置了不止一个；设置了 `source.url` 却没设 `sha256`；`sha256` 不是恰好 64 位十六进制字符；`source.inline` 不是合法 base64；`timeoutMs` 为 `0` 或超过 `10000`；`instancePoolSize` 为 `0` 或超过 `64`。

### Q：模块在本地能跑，到网关里却因为 "load" fail-close？

检查 source 形式：`file` 路径相对网关工作目录解析（相对路径不会相对你本地 shell 的 cwd 解析）；`url` source 要求 URL 在 10 秒内以 `2xx` 状态返回、且字节内容与 `sha256` 匹配。

### Q：guest 能读到前一个请求阶段钩子设置的 header 吗？

在同一次钩子调用内可以——guest 自己的写入对同一次调用内后续的 `proxy_get_header_map_*` 调用可见。但跨请求-header 钩子和响应-header 钩子（同一个请求的两个阶段）则不行——见[无跨阶段 guest 状态](#v1-限制)。

## 相关链接

- [ExtProc 插件](./ext-proc.md) —— 基于 gRPC 的替代方案，相对进程内 Wasm 执行
- [过滤器总览](../README.md)
