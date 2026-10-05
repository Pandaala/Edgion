# 沙盒安全模型

本文档描述 EdgionDSL 脚本执行环境的安全机制，包括资源限制、错误处理策略和各阶段的 API 可用性。

## 安全设计理念

EdgionDSL 的沙盒设计遵循以下核心原则：

**最小能力原则**：脚本在初始状态下没有文件系统访问、没有环境变量读取、没有进程管理操作。所有与网关上下文的交互（读取请求头、拒绝请求等）都必须通过经过审计的内置函数完成，这些函数的行为是可预期且有边界的。出站 HTTP 访问通过 `http.call()` 提供，受到超时限制、响应体大小限制以及可选的 SSRF 防护（`httpBlockPrivateIps`）。Redis 访问则通过 `redis.*` 使用显式配置的 LinkSys 引用。

**纵深防御**：安全保障分为两层。编译期校验在脚本加载时静态检查指令数、变量数、常量池大小等结构性限制，拒绝编译过于复杂或恶意构造的脚本；运行时限制在每次请求执行时动态追踪执行步数、循环迭代次数、函数调用次数等，防止脚本在运行时超出预期资源消耗。两层保护相互补充，单层失效不会导致整体失守。

**Panic 隔离**：`execute_safe` 在异步 VM 执行边界捕获展开式 panic，丢弃 panic 载荷，仅记录固定分类 `vm panic`，然后应用配置的 `errorPolicy`。

**可观测性**：运行时失败在 `PluginLog` 中使用固定分类，并通过请求访问日志输出。VM 失败条目不包含 panic 载荷、脚本名称或触发值。例如，API 失败记录 `api error`，请求体硬失败记录 `request body access error`。

## 运行时资源限制

以下限制在每次请求执行时独立计算，互不干扰。所有配置字段均位于插件的 `config` 块中。

| 限制 | 配置字段 | 默认值 | 最小值 | 最大值 | 说明 |
|------|----------|--------|--------|--------|------|
| 最大执行步数 | `maxSteps` | 10,000 | 1 | 1,000,000 | 每次执行的总字节码指令数 |
| 最大循环迭代次数 | `maxLoopIterations` | 100 | 1 | 100,000 | 单个循环体的迭代上限 |
| 最大函数调用次数 | `maxCallCount` | 500 | 1 | 100,000 | 内置函数调用总数 |
| 最大栈深度 | `maxStackDepth` | 128 | 1 | 1,024 | 值栈深度 |
| 最大字符串长度 | `maxStringLen` | 8,192 | 1 | 1,048,576 | 运行时字符串及单次 Body.read 请求的最大字节数 |
| 动态 List 长度 | `maxListLen` | 1,024 | 1 | 16,384 | 受此限制控制的动态 List 构造可产生的最大元素数 |
| 动态 Map 大小 | `maxMapSize` | 512 | 1 | 4,096 | 受此限制控制的动态 Map 构造和投影可产生的最大条目数 |
| 最大 body 扫描次数 | `maxBodyScanPasses` | 4 | 1 | 16 | 累计扫描原始字节预算，以解析后的物理请求 body 上限计 |
| HTTP 调用超时 | `httpMaxTimeoutMs` | 5,000 毫秒 | 1 毫秒 | 60,000 毫秒 | 单次出站调用的最大墙钟超时 |
| HTTP 请求 Body | `httpMaxRequestBodyBytes` | 1 MiB | 1 字节 | 32 MiB | 单次出站调用可发送的不透明请求 Body 上限 |
| HTTP 响应体 | `httpMaxResponseBodyBytes` | 1 MiB | 1 字节 | 1 MiB | 单次出站调用可保留的响应字节上限 |

VM 还设有不可配置的固定 Value 限制：Map 最多嵌套 64 层；单个 Value 内
Map 条目与 List 元素总计最多 32,768 个；Map 键的 UTF-8 字节数总计最多
262,144 字节。每次执行另有 16 MiB 等价工作预算，用于 Value 结构检查、
复制和相等比较。每个容器元素计 32 单位，每个遍历或复制的 Map 键字节或比较的
字符串字节计 1 单位。这些限制独立于 `maxMapSize`、`maxListLen` 和
操作数栈限制 `maxStackDepth`。VM 在值入栈时检查，并在构造 VM 内的 Map
前预检。宿主内置函数可能在入栈检查前已分配返回值；这些限制不约束该阶段
的聚合内存。

两处外部结果生产路径另有固定的 1 MiB 字段／名称加值字节上限：
`redis.hgetall` 在转换为独立字符串前检查原始回复，`http.call` 在复制每条
响应头前检查当前保留数据。这是单个结果的负载上限，不含容器开销，也不限制
fred 在更早阶段聚合完整 Redis 回复的内存。详见
[Redis API 边界](./api-reference.md#redishgetall) 和
[HTTP 返回值契约](./api-reference.md#httpcall)。

### 各限制详细说明

**maxSteps（最大执行步数）**

每条字节码指令消耗一个步数。步数是对脚本整体计算量最直接的衡量，可以防止无限循环和过度计算。对于典型的请求处理脚本（读取几个头、做条件判断、设置一个头），通常消耗不超过几十步，默认值 10,000 足以覆盖绝大多数场景。如果步数耗尽，VM 抛出 `StepLimitExceeded` 错误。

**maxLoopIterations（最大循环迭代次数）**

对单个 `for` 或 `while` 循环的迭代次数设置上限。例如遍历一个列表时，如果列表元素超过该值，循环会提前终止并抛出 `LoopLimitExceeded` 错误。执行 `continue` 的迭代也会计数。该限制与 `maxSteps` 协同工作：即使步数未耗尽，单个循环过多的迭代也会被拦截。

**maxCallCount（最大函数调用次数）**

计算整个执行过程中内置函数的调用总次数（包括 `req.header()`、`resp.set_header()`、`json_get()` 等所有内置函数）。该限制防止脚本通过大量函数调用产生放大效应，例如在循环中反复调用开销较高的 API。

**maxStackDepth（最大栈深度）**

VM 使用栈来计算表达式。深度嵌套的表达式（如多层函数调用组合）会消耗更多栈空间。该限制防止恶意构造的深层嵌套表达式耗尽内存。对于正常的业务脚本，128 层栈深度远超实际需要。

**maxStringLen（最大字符串长度）**

限制字符串拼接操作产生的结果最大字节数。如果拼接后的字符串超过该值，VM 抛出 `StringTooLong` 错误。该限制同时也是 `json_get()` 等处理字符串数据的函数的间接约束。注意：该限制针对运行时产生的字符串，不影响编译期常量池中的字符串（由单独的编译期限制控制）。

它不限制不透明 Body 本身，但会限制 `Body.read` 以及选中或
序列化后的 JSON 字符串结果。
每次 `Body.read(offset, max_bytes)` 还要求 `max_bytes <= maxStringLen`，默认
8 KiB，可配置到最高 1 MiB。参数超限会在预留扫描预算或 EOF 裁剪之前产生
`StringTooLong`，不会静默缩小参数。
Body 文本读取在 lossy UTF-8 解码过程中、每次接纳输出追加或容量申请之前检查
限制。无效 UTF-8 的膨胀也计入上限；超限不会返回前缀。该限制不等于整个操作
的内存上限：有界扫描缓冲和临时字符串转换也会占用内存。

**maxBodyScanPasses（请求 body 累计扫描次数）**

一次脚本执行的累计扫描预算为网关解析后的物理请求 body 上限乘以
`maxBodyScanPasses`。只读元数据的 `byte_len()` 不消耗扫描字节。扫描以最大
64 KiB 的 source chunk 读取数据，并将每个已读取 chunk 全部计入预算；因此，即使
early-exit 谓词或文本超限读取在最终 chunk 开头停止，也可能计入完整的最终 chunk。
扫描返回结果时会退还未读取字节的预算，文本超限结果也如此。若扫描失败且没有
返回计费信息，则保守保留预留的预算。内存、溢写文件和修改分段使用同一已读取
字节计费规则；不同分段边界可能导致不同的提前停止费用。

`req.body.sha256()` 只在当前不可变 Body 版本上延迟缓存 32 字节摘要。第一次成功
调用正常扫描，同一版本的后续调用不消耗扫描字节。任何成功的 Body 修改都会发布
一个摘要缓存为空的新版本。失败的扫描和失败的修改都不会填充新的缓存项。

`req.body.md5()` 和 `req.body.hmac_sha256(key)` 不缓存，每次调用都消耗实际扫描
字节。MD5 仅用于遗留兼容，包括 FIPS build。Body HMAC 不新增持久 key/MAC 缓存，
并且只用于生成，不用于入站验证：普通 DSL 相等比较不是 constant-time。

`maxListLen` 和 `maxMapSize` 限制运行时集合构造，包括字符串拆分、响应头投影和
Redis hash 投影。零值不是关闭开关：每个运行时限制都必须至少为一。
即使 `maxMapSize` 更小，固定三字段的 HTTP 顶层结果 Map 和 Redis 结果 envelope
仍保留其文档定义的 schema；五字段 `redis.rate_limit` decision 仍要求
`maxMapSize >= 5`。header-name API 继续使用独立的 256 项硬上限，而不使用
`maxListLen`。

三个 `httpMax*` 字段限制一次独立的出站交换。超时复用统一的 60 秒出站上限；
响应保留不能超过 1 MiB，并继续受 `maxStringLen` 的更严格限制；不透明请求 Body
上传继续受网关物理请求 body 上限的更严格限制。

### 配置示例

```yaml
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "my-script"
        source: |
          # 脚本内容
        maxSteps: 5000
        maxLoopIterations: 50
        maxCallCount: 200
        maxStackDepth: 64
        maxStringLen: 4096
        maxBodyScanPasses: 4
        errorPolicy: deny
```

生产环境建议使用默认值。只有在脚本确实需要处理较大数据量或较复杂逻辑时，再针对性地调大单个限制，而不是盲目调大所有限制。

## 编译期校验

脚本在加载时（配置加载期间编译 `source` 或验证 `bytecode`）会执行编译期校验。以下限制在编译阶段静态检查，不通过则拒绝加载脚本。

| 限制 | 默认值 | 说明 |
|------|--------|------|
| 最大指令数 | 1,000 | 编译后的字节码指令总数 |
| 最大局部变量数 | 64 | 局部变量槽位数 |
| 最大常量数 | 256 | 常量池条目数 |
| 单个字符串常量长度 | 4,096 字节 | 单个字符串字面量的最大长度 |
| 常量池总大小 | 32,768 字节 | 常量池所有字符串的总字节数 |
| 最大循环嵌套深度 | 8 | `for` 循环的最大嵌套层级 |
| 正则表达式模式长度 | 1,024 字节 | 单个正则表达式字面量的最大长度 |

预编译字节码也会接受结构化跳转检查：普通跳转不能向后跳或跨越循环边界，循环回跳必须留在所属循环内。无效字节码在加载时被拒绝；实际迭代次数仍在运行时检查。

编译期校验是一次性开销，脚本通过校验后会被缓存，后续请求直接复用编译结果，不重复编译。

正则表达式在编译期不仅检查长度，还会验证语法合法性，不合法的正则会在脚本加载时直接报错，而不是等到运行时才失败。所有正则函数（`.matches()`、`regex_find()`、`regex_replace()`）均要求 pattern 为字符串字面量，动态（变量）pattern 在编译期即被拒绝。正则模式在编译期预编译并缓存，避免每次请求重新编译正则。

## 错误策略（errorPolicy）

`errorPolicy` 控制脚本执行出错时的网关行为。错误包括运行时错误（如类型错误、超限）和 Panic。

```yaml
# 策略一：忽略错误，放行请求（默认）
errorPolicy: ignore

# 策略二：返回 500 错误
errorPolicy: deny

# 策略三：返回自定义状态码和消息体
errorPolicy:
  denyWith:
    status: 403
    body: "Script execution failed"
```

**ignore（默认）**

脚本出错时，`ignore` 记录固定错误分类，并继续执行下一个插件。失败前已完成的头部或上下文修改不会回滚。这种 fail-open 策略适用于非关键增强逻辑，但不能放行 `BodyAccess` 失败。

显式调用 `deny()`、`terminate()` 和 `grpc_error()` 的结果始终按照各自语义处理，不受 `errorPolicy` 影响。

请求体存储、扫描预算、范围、修改以及不透明 Body 上传读取失败在内部归类为 `BodyAccess`。这类错误绕过 `errorPolicy`，固定返回 HTTP 500 拒绝结果，客户端消息为 `Request body processing failed`（gRPC 按正常终止响应规则映射）。该 VM 失败仅记录 `request body access error`，不包含操作细节。这是固定安全边界，不是另一种可配置策略。

**deny**

脚本出错时，网关返回 HTTP 500 并中止请求处理。这是 fail-close 策略，适合安全关键场景（如权限校验、访问控制），确保脚本出错时不会误放行请求。

**denyWith**

与 `deny` 类似，但可以自定义返回的 HTTP 状态码和响应体内容。`status` 必须在 200–599 范围内，`body` 为纯文本字符串。适合需要向客户端返回特定错误格式的场景。

生产环境中，对于安全相关的脚本（权限检查、速率标记等），建议使用 `deny` 或 `denyWith`；对于增强性脚本（添加调试头、设置上下文），可以保留 `ignore`。

## 运行时错误类型

以下错误类型由 VM 在运行时产生。除下文固定硬失败边界 `BodyAccess` 外，其余错误
均根据 `errorPolicy` 处理：

| 错误类型 | 触发条件 | 说明 |
|----------|----------|------|
| `TypeError` | 类型不匹配的操作（如 `"abc" - 1`） | 运算符要求的类型不满足 |
| `DivisionByZero` | 除数为零 | 整数除法除以零 |
| `IntegerOverflow` | 算术溢出 | 运算结果超出 i64 范围 |
| `StepLimitExceeded` | 执行步数超限 | 可能存在无限循环或过度计算 |
| `LoopLimitExceeded` | 循环迭代超限 | 单个循环迭代次数过多 |
| `CallLimitExceeded` | 函数调用超限 | 内置函数调用次数过多 |
| `StackOverflow` | 栈深度超限 | 表达式嵌套过深 |
| `MapTooDeep` | Map 嵌套超过 64 层 | 递归 Map 嵌套过深 |
| `ValueTooManyItems` | Map 条目与 List 元素合计超过 32,768 个 | 容器总体增长过大 |
| `ValueKeysTooLong` | Map 键合计超过 262,144 字节 | Map 键总体过大 |
| `ValueWorkLimitExceeded` | Value 工作量超过固定 16 MiB 等价预算 | 重复遍历、复制或比较开销过大 |
| `StringTooLong` | 字符串过长 | 字符串拼接结果超过 `maxStringLen` |
| `RegexError` | 正则错误 | 正则解析或执行失败 |
| `ApiError` | API 调用或参数校验失败 | 包括 PluginSession 异常、`http.call()` 的 URL/方法/Header 无效，或直接 IP 字面量被 SSRF 预检查拒绝；连接时被拒绝的 DNS 解析地址返回 `status = 0` |
| `BodyAccess` | 请求体存储、扫描、范围、修改或不透明 Body 上传读取失败 | 绕过 `errorPolicy`，固定返回通用 fail-close 响应 |

`TypeError` 是最常见的运行时错误，通常由脚本逻辑处理了可能为 `nil` 的值（如请求头不存在时对其进行字符串操作）。建议在脚本中对从外部读取的值（`req.header()`、`ctx.get()` 等）进行 `nil` 检查。

`ApiError` 包括网关接口失败和无效参数，不一定是临时错误。它遵循 `errorPolicy`，VM 失败日志仅保留固定分类 `api error`。

出站 `http.call()` 的传输或响应流失败、响应体超限以及受限响应转换失败，均返回原子失败 Map `{status: 0, body: "", headers: {}}`。不会返回部分响应，也不会触发 `errorPolicy`。响应体上限为 `min(httpMaxResponseBodyBytes, maxStringLen)`。不透明请求 `Body` 的读取或上传计量失败则不同：它会产生 `BodyAccess`，始终 fail-close。返回值契约见 [http.call](./api-reference.md#httpcall)。

## Panic 隔离

`execute_safe` 在两个执行阶段都使用异步 `catch_unwind` 边界包裹完整 VM Future。捕获的展开式 panic 不会传播给调用方。panic 载荷会被丢弃；该边界唯一记录的 panic 诊断是在 `PluginLog` 中写入 `vm panic`，随后应用配置的 `errorPolicy`。它不会附加 panic 消息、脚本名称或触发细节。

此边界是兜底保护，不能替代可失败内置函数应返回的类型化错误。它捕获展开式 panic，不捕获进程中止。

## 正则表达式安全

DSL 的正则表达式功能在设计上防范 ReDoS（正则拒绝服务）攻击：

**编译期预编译**：`.matches()` 使用的正则模式在脚本编译期构建为前向稠密 DFA。DFA 最大 1 MiB，并增量消费请求体，因此匹配不需要完整物化请求体。

**语法校验**：不合法的正则模式会在编译期报错，不会延迟到运行时才失败。这样可以在脚本加载阶段就发现正则错误，而不是在生产流量中才暴露问题。

**长度和自动机限制**：正则模式最大 1,024 字节，编译后的 DFA 最大 1 MiB。引擎不使用回溯，请求体输入消耗还受 `maxBodyScanPasses` 限制。流式匹配器不支持 Unicode 感知的单词边界；适用时可显式使用 ASCII 边界 `(?-u:\b)`。

`regex_find()` 和 `regex_replace()` 函数的模式参数同样受 1,024 字节限制约束。

## 执行阶段限制

DSL 插件可以在两个阶段运行，不同阶段的可用 API 有所不同：

| 阶段 | 可用 API | 不可用 API | 说明 |
|------|---------|-----------|------|
| `requestPlugins` | `req.*`、`ctx.*`、`deny`、`next`、`terminate`、`grpc_error` | `resp.set_status`（调用返回 `nil`，无实际效果） | 请求转发前执行 |
| `upstreamResponseFilterPlugins` | `req.*`（只读）、`resp.*`、`ctx.*`、`deny`、`next`、`grpc_error`、`terminate` | 无 | 响应头阶段执行 |

在响应阶段仍可调用 `req.*`，但请求体读取只能使用请求阶段已经捕获的请求体；响应阶段的延迟读取不会重新开始从下游捕获。请求体修改方法在 `requestPlugins` 以外的阶段会在配置校验时被拒绝；`req.set_header()` 等其他请求修改也无法改变已经转发的请求。

在 `requestPlugins` 中，`grpc_error()` 通过统一终止响应写入器直接发送 trailers-only gRPC 响应。在 `upstreamResponseFilterPlugins` 中，它会中止上游响应，将准确的 gRPC 状态码和可选消息传递给同一写入器；上游响应不会继续转发。两个阶段的线缆 HTTP 状态均为 `200`，gRPC 应用状态则在访问日志的 `grpc_status` 中独立记录，不会从 HTTP 状态码重新推导。

## 函数级别限制汇总

部分内置函数除了受全局资源限制约束外，还有自身的参数或结果限制：

| 函数 | 限制 | 说明 |
|------|------|------|
| `req.body.json_get` | 确定性 JSONPath 最大 1,024 字节、32 段；选中结果受 `maxStringLen` 限制 | 选择性解析单个值，不构建完整 JSON DOM |
| `req.body.json_select` | 1–64 个静态命名路径；名称最多 256 字节；单值和实时聚合字符串受 `maxStringLen` 限制 | 一次完整解析和 Body 扫描选择多个值 |
| `req.body.read` | 范围必须非负；请求的原始字节数和输出分别受 `maxStringLen` 限制 | 透明读取内存或磁盘缓存的请求体 |
| `req.body.matches` / 其他请求体扫描 | 总扫描字节数受 `maxBodyScanPasses × 请求体物理上限` 限制 | 防止重复完整扫描请求体 |
| 请求体修改方法 | 结果受请求体物理上限和 4,096 个修改片段限制 | 修改具有事务性，且仅允许在请求阶段 |
| `time_format` | 格式字符串最大 256 字符 | 防止格式字符串过长导致的资源消耗 |
| `regex_find` / `regex_replace` | 模式最大 1,024 字节 | 防止 ReDoS |
| `req.header_names` / `resp.header_names` | 最多返回 256 个头名称 | 防止大量请求头导致的内存问题 |
| `resp.set_status` | 最终状态码范围 200–599 | 超出范围时返回 `ApiError` |
| `range` | 最多生成 1,000 个元素 | 防止大列表占用过多内存 |
| `http.call` | 超时上限 `httpMaxTimeoutMs`（默认 5,000 ms）；不透明 `Body` 上限为 `min(httpMaxRequestBodyBytes, 物理 body 上限)`（默认 1 MiB，配置最大 32 MiB）；响应体上限 `httpMaxResponseBodyBytes`（默认 1 MiB）；按配置拦截私有 IP | 防止网关资源耗尽和 SSRF 攻击 |

以 `req.body` 作为 `http.call` 第三个参数时，运行时从调用时的不可变快照拉取流式
发送原始字节，不会完整物化请求体。实际拉取的字节计入累计 body 扫描预算。调用方
不能设置 `Content-Length` 或 `Transfer-Encoding`；空 `Body` 发送
`Content-Length: 0`，`nil` 不附加 body。不透明 `Body` 调用直接返回 3xx，不跟随
重定向；`Str` 和 `Nil` 调用保留现有重定向行为。

`req.body.json_get()` 只接受以 `$` 为根的确定性路径：`.member`、`["quoted member"]` 和非负 `[index]` 段。通配符、递归下降、过滤器、切片、联合和动态路径会在编译期被拒绝。

`req.body.json_select()` 接受一个非空 Map 字面量，将唯一输出名映射到相同的确定性
路径。共享前缀计划在 VM 初始化阶段构建，而不是逐请求构建。需要多个字段时，应使用
它代替重复调用 `json_get`。

`req.header_names()` 和 `resp.header_names()` 返回一个字符串列表。当实际头数量超过 256 时，只返回前 256 个，不产生错误。如果需要遍历所有头，应当结合具体的头名称使用 `req.header()` 直接访问。

## 相关文档

- [API 参考手册](./api-reference.md) — 所有内置函数的详细签名和用法
- [语言语法参考](./language-syntax.md) — 类型系统、运算符、控制流
- [实战示例集](./examples.md) — 包含安全相关的实际脚本示例
- [字节码指南](./bytecode.md) — 预编译字节码的使用方法（可规避编译期开销）
