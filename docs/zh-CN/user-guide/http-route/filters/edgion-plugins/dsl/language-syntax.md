# EdgionDSL 语言语法参考

本文档描述 EdgionDSL 的完整语法规范。EdgionDSL 是专为 Edgion 网关插件设计的轻量级脚本语言，运行于安全沙箱中，用于实现请求/响应的自定义处理逻辑。

---

## 1. 类型系统

EdgionDSL 拥有七种内置值类型：

| 类型 | 说明 | 示例字面量 |
|------|------|-----------|
| `String` | UTF-8 字符串 | `"hello"` |
| `Int` | 64 位有符号整数 | `42`、`-1` |
| `Bool` | 布尔值 | `true`、`false` |
| `Nil` | 空值，表示"无值" | `nil` |
| `Body` | 当前被代理请求 body 的不透明句柄 | 由 `req.body` 返回 |
| `List` | 字符串列表，用于迭代（只读） | 由 `req.header_names()` 等 API 返回 |
| `Map` | 字符串键、任意类型值的有序映射表 | `{"key": "value"}`、`{"status": 200}` |

`List` 类型仅作为迭代容器使用，不能用字面量直接构造，也不能直接与其他类型比较。

`Map` 类型保存一组有序的键值对，键始终为 `String`，值可以是任意 DSL 类型（包括嵌套 `Map`）。通过索引访问读取值（`map["key"]`）；键不存在时返回 `nil`。`Map` 目前主要由 `http.call()` 返回，也可通过 `{key: expr, ...}` 字面量直接构造扁平 Map。

### 1.1 类型转换

调用 `to_str(val)` 或在字符串拼接中隐式触发转换时，各类型的行为如下：

| 源类型 | 转换结果 |
|--------|---------|
| `Int` | 十进制字符串，例如 `42` → `"42"`，`-1` → `"-1"` |
| `Bool` | `true` → `"true"`，`false` → `"false"` |
| `Nil` | `"nil"` |
| `List` | `"[list]"`（固定字符串，不展开内容） |
| `Map` | `"[map]"`（固定字符串，不展开内容） |
| `String` | 不变 |
| `Body` | **拒绝转换**；必须显式使用有界 Body 读取 |

```dsl
let n = 100
let s = to_str(n)      // s == "100"

let b = false
let t = to_str(b)      // t == "false"

let x = nil
let y = to_str(x)      // y == "nil"
```

### 1.2 真值规则

EdgionDSL 采用 Lua 风格的真值语义：

- **假值**：`Nil` 和 `false`
- **真值**：所有其他值，包括 `0`、空字符串 `""`、空列表和 `Body`

```dsl
// 以下条件均为真（进入 if 分支）
if 0 { ... }
if "" { ... }

// 以下条件均为假（不进入 if 分支）
if nil { ... }
if false { ... }
```

> 注意：空字符串 `""` 在 EdgionDSL 中是真值，这与 Python 等语言不同。检查字符串是否为空时，应使用 `len(s) == 0`。

### 1.3 跨类型相等

不同类型的值通常不相等，即使内容看起来相同。`Body`/`String` 是唯一例外：
无论左右顺序，都按流比较逻辑文本。两个 Body 都标识当前请求 body，因此相等。

```dsl
1 == "1"       // false，Int != String
nil == false   // false，Nil != Bool
0 == false     // false，Int != Bool
req.body == "expected"  // Body/String 流式相等比较
```

---

## 2. 变量

### 2.1 变量声明

EdgionDSL 使用 `let` 关键字声明变量。默认声明为不可变变量，添加 `mut` 修饰符声明可变变量：

```dsl
// 不可变变量
let name = "alice"
let code = 200

// 可变变量
let mut counter = 0
let mut message = "init"
```

### 2.2 赋值

只有用 `let mut` 声明的变量才能被重新赋值。对不可变变量赋值会产生编译错误：

```dsl
let x = 10
x = 20          // 编译错误：x 不可变

let mut y = 10
y = 20          // 合法
y = y + 5      // y == 25
```

### 2.3 作用域

变量的作用域从声明处开始，到所在代码块（`{}`）结束。内层块可以访问外层块的变量，外层块不能访问内层块声明的变量：

```dsl
let token = req.header("Authorization")

if token != nil {
    let mut prefix = "Bearer "
    // token 和 prefix 均可访问
    if token.starts_with(prefix) {
        let cleaned = substr(token, 7, len(token))
        // token、prefix、cleaned 均可访问
    }
    // cleaned 在此处不可访问
}
// prefix 在此处不可访问
```

---

## 3. 运算符

### 3.1 算术运算符

| 运算符 | 说明 | 示例 |
|--------|------|------|
| `+` | 加法（Int）或字符串拼接（String） | `1 + 2`，`"a" + "b"` |
| `-` | 减法 | `10 - 3` |
| `*` | 乘法 | `4 * 5` |
| `/` | 整数除法（截断） | `7 / 2 == 3` |

**整数除法截断**：结果向零截断，不进行四舍五入：

```dsl
7 / 2    // 3（非 3.5）
-7 / 2   // -3（非 -4）
```

**除以零**：产生运行时 `DivisionByZero` 错误。脚本会根据 `errorPolicy` 配置决定后续行为。

**字符串拼接**：`+` 运算符两侧只要有一侧是 `String`，另一侧会自动调用
转换为文本后拼接。`Body` 被明确排除；必须显式使用 `read(offset, max_bytes)`：

```dsl
"count: " + 42           // "count: 42"
"flag: " + true          // "flag: true"
"value: " + nil          // "value: nil"
"hello" + " " + "world"  // "hello world"
```

### 3.2 比较运算符

| 运算符 | 说明 |
|--------|------|
| `==` | 相等 |
| `!=` | 不相等 |
| `<` | 小于 |
| `>` | 大于 |
| `<=` | 小于等于 |
| `>=` | 大于等于 |

大小比较（`<`、`>`、`<=`、`>=`）接受类型匹配的 `Int`/`Int` 或
`String`/`String` 操作数。Body 或类型不匹配的组合会产生运行时类型错误。

字符串相等比较使用精确字节值，字符串大小比较使用字典序：

```dsl
"abc" == "abc"   // true
"abc" != "def"   // true
100 > 50         // true
100 < 200        // true
```

### 3.3 逻辑运算符

| 运算符 | 说明 |
|--------|------|
| `&&` | 逻辑与（短路） |
| `\|\|` | 逻辑或（短路） |
| `!` | 逻辑非（一元） |

**短路求值**：

- `&&`：左侧为假时，右侧表达式不会被求值
- `||`：左侧为真时，右侧表达式不会被求值

**结果类型**：无论操作数是什么类型，`&&` 和 `||` 始终求值为 `Bool`（`true` / `false`）。它们是逻辑运算符，不是取值 / 空值合并运算符——`false || "x"` 得到 `true`，而不是 `"x"`。若需根据条件选择某个值，请使用 `if`/`else` 块。

```dsl
let h = req.header("X-Token")

// 短路：h == nil 为真时，不会调用 h.starts_with()
if h == nil || h.starts_with("Bearer ") {
    return deny(401, "invalid token format")
}
```

### 3.4 运算符优先级

从高到低排列：

| 优先级 | 运算符 | 说明 |
|--------|--------|------|
| 1（最高） | `!`、`-`（一元） | 逻辑非、取负 |
| 2 | `*`、`/` | 乘法、除法 |
| 3 | `+`、`-` | 加法、减法 |
| 4 | `<`、`>`、`<=`、`>=` | 大小比较 |
| 5 | `==`、`!=` | 相等比较 |
| 6 | `&&` | 逻辑与 |
| 7（最低） | `\|\|` | 逻辑或 |

使用括号可以显式控制求值顺序：

```dsl
// 等价：(a || b) && c
let result = a || b && c    // && 优先于 ||，先算 b && c

// 使用括号改变顺序
let result2 = (a || b) && c
```

---

## 4. 控制流

### 4.1 if / else if / else

条件分支语句，根据条件的真值决定执行路径：

```dsl
if condition {
    // condition 为真时执行
} else if other_condition {
    // other_condition 为真时执行
} else {
    // 所有条件均为假时执行
}
```

完整示例：

```dsl
let method = req.method()

if method == "GET" {
    req.set_header("X-Method-Type", "read")
} else if method == "POST" || method == "PUT" {
    req.set_header("X-Method-Type", "write")
} else if method == "DELETE" {
    let admin = req.header("X-Admin-Token")
    if admin == nil {
        return deny(403, "DELETE requires admin token")
    }
    req.set_header("X-Method-Type", "delete")
} else {
    return deny(405, "method not allowed")
}
```

条件表达式不需要括号，但代码块必须使用大括号 `{}`。

### 4.2 for...in（迭代循环）

遍历 `List` 类型中的每个元素。`List` 由 API 调用（如 `req.header_names()`、`split()`、`range()`）返回：

```dsl
for name in req.header_names() {
    // name 是当前迭代的请求头名称（String 类型）
    if name.starts_with("x-internal-") {
        req.remove_header(name)
    }
}
```

迭代 `split()` 结果：

```dsl
let path = req.path()
let segments = split(path, "/")

for seg in segments {
    if seg == "admin" {
        return deny(403, "access denied")
    }
}
```

### 4.3 for...in range()（范围循环）

`range(start, end)` 生成从 `start`（含）到 `end`（不含）的整数序列，以字符串形式返回。

> 注意：`range()` 生成的列表中每个元素均为字符串类型（`"0"`、`"1"`），而非整数。

```dsl
// 遍历 0 到 9
for i in range(0, 10) {
    // i 是字符串 "0", "1", ..., "9"
    let header_name = "X-Retry-" + i
    req.remove_header(header_name)
}
```

`range(start, end)` 最多生成 `min(max_list_len, 1000)` 个元素（其中 `max_list_len` 是单个插件 VM 的限制，默认 1024）。超出上限的元素会被静默截断。

### 4.4 while（条件循环）

在条件为真时重复执行循环体：

```dsl
let mut i = 0
let mut total = 0

while i < 10 {
    total = total + i
    i = i + 1
}
// total == 45
```

while 循环同样受 `maxLoopIterations` 配置限制，超出后产生运行时错误。

### 4.5 break 和 continue

- `break`：立即退出当前循环
- `continue`：跳过当前迭代，进入下一次循环

```dsl
let mut found = false

for name in req.header_names() {
    if name == "x-target-header" {
        found = true
        break   // 找到后立即退出循环
    }
}

// continue 示例
for name in req.header_names() {
    if !name.starts_with("x-forward-") {
        continue   // 跳过不匹配的头
    }
    req.remove_header(name)
}
```

`break` 和 `continue` 仅影响最近一层的循环，不支持跨层跳转。

---

## 5. 文本与 Body 方法

字符串和 Body 共享以下点号方法。调用者或参数类型错误时，谓词方法保留现有
nil-friendly 行为，返回 `false`：

| 方法 | 参数 | 返回值 | 说明 |
|------|------|--------|------|
| `.len()` | 无 | Int | String 或 lossy 解码后的 Body 文本 Unicode 标量数量 |
| `.is_empty()` | 无 | Bool | String 或 Body 文本是否为空 |
| `.starts_with(prefix)` | String | Bool | 是否以指定前缀开头 |
| `.ends_with(suffix)` | String | Bool | 是否以指定后缀结尾 |
| `.contains(substring)` | String | Bool | 是否包含指定子字符串 |
| `.matches(pattern)` | String | Bool | 是否匹配正则表达式（编译期校验） |

Body 还提供 `.byte_len()`、`.read(offset, max_bytes)`、
`.sha256()`、`.md5()`、`.hmac_sha256(key)`、`.json_get("确定路径")` 和静态多路径
`.json_select({"name": "$.path"})`。请求阶段脚本可用 `.clear()`、
`.append(s)`、`.prepend(s)`、`.remove(start, end)` 和字面量 `.replace(from,
to)` 修改它。边界和失败行为见
[API 参考](./api-reference.md#reqbody)。

```dsl
let ua = req.header("User-Agent")

if ua != nil {
    if ua.starts_with("Mozilla") {
        req.set_header("X-Client-Browser", "true")
    }

    if ua.ends_with("Mobile Safari") {
        req.set_header("X-Client-Mobile", "true")
    }

    if ua.contains("Bot") {
        return deny(403, "bots not allowed")
    }
}
```

`.matches()` 的正则模式在脚本编译时验证，并且必须符合 String 与 Body 共用的
有界流式 DFA 方言。Unicode 单词边界会被拒绝；ASCII 单词边界可用
`(?-u:\b)`。无效或不兼容的模式会导致脚本加载失败：

```dsl
let path = req.path()

// 正则匹配：路径是否符合 /api/v{数字}/ 格式
if path.matches("^/api/v[0-9]+/") {
    req.set_header("X-Versioned-Api", "true")
}
```

---

## 6. 返回语句

脚本必须通过 `return` 语句明确结束，或在脚本末尾隐式结束。

### 6.1 return next()

放行请求，继续执行下一个插件。这是最常用的返回方式：

```dsl
// 添加追踪头后继续
req.set_header("X-Trace-Id", uuid())
return next()
```

### 6.2 return deny(status, body)

拒绝请求，返回指定的 HTTP 状态码和响应体：

- `status`：整数，最终响应范围 200-599
- `body`：字符串或 `nil`（nil 表示无响应体）

```dsl
let token = req.header("Authorization")
if token == nil {
    return deny(401, "Authorization header is required")
}

// 无响应体
return deny(204, nil)
```

### 6.3 return terminate()

立即停止脚本及后续插件处理，显式返回 HTTP `502` 和空响应体；gRPC 请求返回 HTTP `200` 和 `grpc-status: 13`（INTERNAL）。这不是静默关闭连接，`errorPolicy: ignore` 也不会放行。需要自定义拒绝状态时使用 `deny()` 或 `grpc_error()`。响应阶段和连接行为见 [terminate API](./api-reference.md#terminate)。

```dsl
// 该精确网络来源规则故意匹配物理传输对端。
let ip = req.direct_peer_ip()
if ip == "1.2.3.4" {
    return terminate()
}
```

### 6.4 return grpc_error(code, message)

返回原生 gRPC 错误，通过 HTTP/2 trailers 传递标准 gRPC 状态码：

- `code`：整数，范围 0-16（超出范围截断为 13 即 INTERNAL）
- `message`：人类可读的错误描述字符串

```dsl
if req.protocol != "grpc" {
    return next()
}

let token = req.header("authorization")
if token == nil {
    return grpc_error(16, "missing authorization token")  // UNAUTHENTICATED
}

let service = req.grpc_service
if service == "internal.AdminService" {
    return grpc_error(7, "permission denied")  // PERMISSION_DENIED
}
```

> `requestPlugins` 和 `upstreamResponseFilterPlugins` 均支持此操作。请求阶段直接写入 trailers-only 响应；响应阶段中止上游响应，并将准确的状态码和可选消息保留给统一终止响应写入器。线缆 HTTP 状态保持 `200`，`grpc_status` 独立记录 gRPC 应用状态。详见 [DSL 插件 — gRPC 错误返回](./README.md#grpc-错误返回)。

### 6.5 隐式返回

如果脚本执行到末尾没有显式 `return` 语句，默认行为等同于 `return next()`：

```dsl
// 无 return 语句，等同于 return next()
let ua = req.header("User-Agent")
if ua != nil {
    req.set_header("X-Has-UA", "true")
}
// 脚本结束，自动 next()
```

---

## 7. 字符串字面量

字符串使用双引号 `"` 包围：

```dsl
let greeting = "hello, world"
let empty = ""
```

支持以下转义字符：

| 转义序列 | 含义 |
|----------|------|
| `\n` | 换行符（LF） |
| `\t` | 制表符 |
| `\\` | 反斜杠 `\` |
| `\"` | 双引号 `"` |

```dsl
let multiline_msg = "line one\nline two"
let path_sep = "C:\\Users\\alice"
let quoted = "she said \"hello\""
```

普通双引号字符串也可以直接包含实际换行；引号内的换行会保留在字符串值中。下面的写法与上面的 `multiline_msg` 具有相同的值：

```dsl
let literal_newline_msg = "line one
line two"
```

语言没有专用的原始字符串（raw string）或三引号语法。普通双引号字符串跨行时仍会处理转义序列。复杂内容也可以使用转义序列或字符串拼接。

---

## 8. Map 字面量与索引访问

### 8.1 Map 字面量

使用 `{key: expr, ...}` 语法构造扁平映射表（字符串键，任意类型值）。键必须是字符串字面量，值可以是任意表达式：

```dsl
let hdrs = {"Content-Type": "application/json", "X-Api-Key": "secret"}
let meta = {"retries": 3, "enabled": true}
```

Map 字面量主要用于向 `http.call()` 传递可选的 `headers` 参数。

### 8.2 索引访问

使用 `expr[key]` 读取 `Map` 中的值：

- `key` 必须是 `String` 表达式。键不存在时返回 `nil`。
- `List` 不支持直接索引访问，请使用 `for...in` 迭代。

```dsl
let r = http.call("GET", "https://api.example.com/status", nil)

// 读取顶层字段
let code = r["status"]          // Int

// 读取嵌套 Map 字段（响应头以 Map 形式返回）
let ct = r["headers"]["content-type"]   // Str | Nil

if code == 200 {
  req.set_header("X-Backend-Ok", "true")
}
```

`len()` 作用于 `Map` 时返回键值对数量：

```dsl
let m = {"a": 1, "b": 2, "c": 3}
let n = len(m)    // 3
```

---

## 9. 注释

EdgionDSL 支持单行注释，以 `//` 开始，到行末结束：

```dsl
// 这是一个单行注释

let token = req.header("Authorization")  // 获取认证头

// 多行逻辑可以分段注释
// 第一步：校验必要字段
if token == nil {
    return deny(401, "missing token")
}

// 第二步：放行请求
return next()
```

不支持块注释（`/* ... */`）。

---

## 10. 综合示例

### 示例 1：多条件请求校验

```dsl
// 校验方法、Token 和路径前缀
let method = req.method()
let token = req.header("X-Api-Key")
let path = req.path()

if token == nil {
    return deny(401, "X-Api-Key header is required")
}

if method == "DELETE" && !path.starts_with("/api/v2/") {
    return deny(403, "DELETE is only allowed on /api/v2/ resources")
}

// 记录协议类型
let proto = req.protocol
if proto == "grpc" {
    req.set_header("X-Protocol", "grpc")
} else {
    req.set_header("X-Protocol", "http")
}

return next()
```

### 示例 2：遍历并清理内部头

```dsl
// 删除所有 x-internal-* 请求头，防止客户端注入内部路由信息
for name in req.header_names() {
    if name.starts_with("x-internal-") {
        req.remove_header(name)
    }
}

return next()
```

### 示例 3：带计数器的循环

```dsl
// 统计以 "x-tag-" 开头的请求头数量，超过 10 个则拒绝
let mut tag_count = 0

for name in req.header_names() {
    if name.starts_with("x-tag-") {
        tag_count = tag_count + 1
    }
    if tag_count > 10 {
        break
    }
}

if tag_count > 10 {
    return deny(400, "too many x-tag-* headers")
}

return next()
```

### 示例 4：gRPC 服务路由控制

```dsl
// 仅允许特定 gRPC 服务通过
if req.protocol != "grpc" {
    return next()
}

let service = req.grpc_service

if service == nil {
    return grpc_error(3, "cannot determine gRPC service name")  // INVALID_ARGUMENT
}

// 白名单校验
let allowed = false
if service == "user.UserService" {
    allowed = true
}
if service == "order.OrderService" {
    allowed = true
}

if !allowed {
    return grpc_error(12, "service not available: " + service)  // UNIMPLEMENTED
}

return next()
```

### 示例 5：IP 范围检查

```dsl
// 该内网来源豁免故意使用物理传输对端。
let ip = req.direct_peer_ip()

// 内网 IP 允许直接通过，无需 Token
if ip_in_cidr(ip, "10.0.0.0/8") || ip_in_cidr(ip, "172.16.0.0/12") {
    req.set_header("X-Internal-Request", "true")
    return next()
}

// 外部请求必须携带 Token
let token = req.header("Authorization")
if token == nil {
    return deny(401, "Authorization required for external requests")
}

return next()
```

---

## 相关文档

- [DSL 插件配置参考](./README.md) — 配置参数、API 列表、错误策略
- [过滤器总览](../../overview.md)
- [插件组合与引用](../../plugin-composition.md)
