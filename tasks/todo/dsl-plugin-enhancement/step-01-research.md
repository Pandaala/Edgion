# EdgionDSL 插件增强研究报告

**日期**：2026-03-22
**作者**：研究分析（基于源码深度阅读）
**源码位置**：`src/core/gateway/plugins/http/dsl/`

---

## 1. 当前能力清单

### 1.1 数据类型

| 类型 | 状态 | 说明 |
|------|------|------|
| `Str` (String) | 已实现 | 主类型，所有 HTTP 数据均为字符串 |
| `Int` (i64) | 已实现 | 用于状态码、长度、计数器 |
| `Bool` | 已实现 | 条件判断结果 |
| `Nil` | 已实现 | 缺失/空值，映射 Option::None |
| `List` (Vec<String>) | 已实现（内部） | 仅用于 for-in 迭代，不可用户构造 |
| `Float` | **未实现** | - |
| `Map` / `Object` | **未实现** | - |
| 用户自定义 List | **未实现** | 用户无法创建列表字面量 |

**注意**：空字符串（`""`）和整数 0 均视为 truthy（Lua 风格），这与 Python/JS 不同，可能造成用户混淆。

### 1.2 运算符

| 类型 | 支持 |
|------|------|
| 算术：`+` `-` `*` `/` | 已实现（Int only，`+` 支持字符串拼接） |
| 取反：`-` | 已实现（Int only） |
| 比较：`==` `!=` `<` `>` `<=` `>=` | 已实现（Int/Int 或 Str/Str，跨类型返回 false 不报错） |
| 逻辑：`&&` `\|\|` `!` | 已实现（短路求值） |
| 取模 `%` | **未实现** |
| 位运算 | **未实现** |

### 1.3 字符串操作

| 操作 | 状态 |
|------|------|
| `s.starts_with(prefix)` | 已实现 |
| `s.ends_with(suffix)` | 已实现 |
| `s.contains(substr)` | 已实现 |
| `s.matches("regex")` | 已实现（pattern 必须是字面量，编译期校验） |
| `+` 拼接 | 已实现（有 max_string_len 保护） |
| `len(s)` | 已实现（UTF-8 字符数） |
| `substr(s, start, end)` | 已实现（UTF-8 安全） |
| `to_upper(s)` / `to_lower(s)` | 已实现 |
| `to_str(v)` / `to_int(s)` | 已实现 |
| `base64_encode(s)` / `base64_decode(s)` | 已实现 |
| `url_encode(s)` / `url_decode(s)` | 已实现 |
| `sha256(s)` | **已声明，未实现**（运行时报 ApiError） |
| `md5(s)` | **已声明，未实现**（运行时报 ApiError） |
| `regex_find(s, pattern)` | **已声明，未实现**（运行时报 ApiError） |
| `regex_replace(s, pattern, replacement)` | **已声明，未实现**（运行时报 ApiError） |
| `trim()` / `split()` / `join()` | **未实现** |

### 1.4 控制流

| 特性 | 状态 |
|------|------|
| `if / else if / else` | 已实现 |
| `while` 循环 | 已实现（有 max_loop_iterations 保护） |
| `for i in range(start, end)` | 已实现（语法层支持，`range()` 运行时**未实现**） |
| `for x in list` | 已实现（for-in 迭代） |
| `break` / `continue` | **未实现** |
| 函数定义（用户自定义函数） | **未实现** |
| 递归 | **未实现** |

**重要 Bug**：`range()` 函数已声明、编译时会通过，但运行时实现缺失（返回 ApiError "range() is not yet implemented"）。这意味着 `for i in range(0, 5)` 语法上合法，运行时会崩溃。

### 1.5 请求上下文访问（读）

| API | 状态 |
|-----|------|
| `req.header("name")` | 已实现 |
| `req.header_names()` | 已实现（返回 List，最多 256 个） |
| `req.has_header("name")` | 已实现 |
| `req.method` | 已实现 |
| `req.path` | 已实现 |
| `req.query("name")` | 已实现（单个查询参数） |
| `req.query_string` | 已实现（完整查询字符串） |
| `req.cookie("name")` | 已实现 |
| `req.client_ip` | 已实现（直接连接 IP） |
| `req.remote_ip` | 已实现 |
| `req.path_param("name")` | 已实现（路由参数，如 `/api/:id`） |
| `req.scheme` | 已实现（从 X-Forwarded-Proto 读取） |
| `req.host` | 已实现（从 Host header 读取） |
| `req.uri` | 已实现（path + query） |
| `req.content_type` | 已实现 |
| 请求 Body 读取 | **未实现** |
| 所有查询参数（Map） | **未实现** |
| 所有 Cookie（Map） | **未实现** |

### 1.6 请求上下文修改（写）

| API | 状态 |
|-----|------|
| `req.set_header("name", "value")` | 已实现 |
| `req.append_header("name", "value")` | 已实现 |
| `req.remove_header("name")` | 已实现 |
| `req.set_uri("/new/path")` | 已实现 |
| `req.set_host("host")` | 已实现 |
| `req.set_method("POST")` | 已实现 |
| 修改查询参数 | **未实现** |
| 修改 Body | **未实现** |

### 1.7 响应上下文操作

| API | 状态 |
|-----|------|
| `resp.set_header("name", "value")` | 已实现 |
| `resp.append_header("name", "value")` | 已实现 |
| `resp.remove_header("name")` | 已实现 |
| `resp.status` 读取 | **未实现** |
| `resp.set_status(code)` | **未实现** |
| 读取响应 Body | **未实现** |
| 修改响应 Body | **未实现** |

### 1.8 跨插件上下文（ctx）

| API | 状态 |
|-----|------|
| `ctx.get("key")` | 已实现（读取插件间共享变量） |
| `ctx.set("key", "value")` | 已实现 |
| `ctx.remove("key")` | 已实现 |

### 1.9 工具函数

| 函数 | 状态 |
|------|------|
| `log("msg")` | 已实现（写入 PluginLog） |
| `time_now()` | 已实现（Unix 时间戳，秒） |
| `sha256(s)` | 已声明**未实现** |
| `md5(s)` | 已声明**未实现** |
| `regex_find(s, pattern)` | 已声明**未实现** |
| `regex_replace(s, pattern, replacement)` | 已声明**未实现** |
| JSON 解析 | **未实现** |
| UUID 生成 | **未实现** |

### 1.10 副作用与终止

| 操作 | 状态 |
|------|------|
| `return next()` 继续处理（隐式） | 已实现 |
| `return deny(status, body)` 终止并返回响应 | 已实现 |
| HTTP 重定向 | **未实现**（无 `resp.redirect()` 接口） |
| 调用外部 HTTP 子请求 | **未实现** |
| 调用 Redis | **未实现** |
| 异步操作 | **未实现**（VM 是同步执行的） |

### 1.11 安全模型（已实现）

- 源码大小限制：64KB
- 解析预算：50,000 次 combinator 调用
- AST 节点上限：2,000 个
- 嵌套深度限制：64 层
- 字节码大小限制：256KB（base64 前）
- VM 步数预算：默认 10,000 步（可配置，最大 1,000,000）
- 每循环迭代上限：默认 100 次（可配置）
- 内置函数调用上限：默认 500 次
- 栈深度限制：默认 128
- 字符串长度限制：默认 8192 字节
- `catch_unwind` 隔离 VM panic
- 正则表达式长度限制：1024 字符，编译期校验
- 字节码版本验证

### 1.12 配置与部署

| 特性 | 状态 |
|------|------|
| 内联源码（YAML `source:`） | 已实现 |
| 预编译字节码（`bytecode:`） | 已实现（base64 JSON） |
| 配置期预编译缓存 | 已实现（避免双重编译） |
| 错误策略：ignore / deny / denyWith | 已实现 |
| VM 限制可配置 | 已实现（per-plugin 配置） |

---

## 2. 能力缺口分析

### 2.1 关键缺口（阻碍实际使用）

#### 缺口 A：`range()` 函数未实现但已暴露

- **现象**：`for i in range(0, 5)` 能编译通过，但运行时抛 ApiError
- **影响**：任何使用数值循环的脚本都会静默失败（按 error_policy 处理）
- **优先级**：P0，必须先修复

#### 缺口 B：`sha256()` / `md5()` / `regex_find()` / `regex_replace()` 已声明未实现

- **现象**：编译期通过，运行时报错
- **影响**：用户看到这些 API 名称后期待能用，但实际不可用
- **优先级**：P1（sha256 是认证场景的刚需）

#### 缺口 C：无法响应修改（resp 只能写 header，不能读 status）

- **现象**：`resp.*` 只支持写 header，无法读取响应状态码
- **影响**：响应阶段的条件逻辑无法实现（如"只在 5xx 时添加错误 header"）
- **优先级**：P1

#### 缺口 D：没有字符串 `split()` / `trim()`

- **现象**：无法分割字符串，无法处理多值 header（如 `Accept: text/html, application/json`）
- **影响**：复杂 header 处理场景无法实现
- **优先级**：P1

#### 缺口 E：`return deny()` 只能返回纯文本

- **现象**：body 参数被转成字符串，无法返回 JSON 结构体
- **影响**：API Gateway 场景下通常需要返回结构化错误（`{"code": 403, "msg": "..."}`）
- **优先级**：P2（可通过手动拼接字符串绕过）

#### 缺口 F：无法读取请求 Body

- **现象**：完全没有 `req.body` 相关 API
- **影响**：基于 body 内容的请求过滤/修改无法实现
- **优先级**：P2（涉及架构改动，Pingora 的 body 访问需要异步）

### 2.2 中等缺口（影响用户体验）

#### 缺口 G：无浮点数类型

- **影响**：无法做比率计算（如限流权重），但 HTTP 场景较少涉及浮点
- **优先级**：P3

#### 缺口 H：无 JSON 解析

- **影响**：无法解析 `Content-Type: application/json` 的请求体或响应体
- **优先级**：P2（依赖缺口 F 解决后才有意义）

#### 缺口 I：无 `break` / `continue`

- **影响**：循环逻辑不够灵活（如找到第一个匹配即退出）
- **优先级**：P2

#### 缺口 J：无用户自定义函数

- **影响**：脚本无法复用逻辑，代码结构扁平
- **优先级**：P3（脚本通常较短，影响有限）

#### 缺口 K：无外部资源访问（Redis / HTTP 子请求）

- **影响**：无法实现"查 Redis 做动态限流"、"调用鉴权服务"等高级场景
- **优先级**：P3（涉及异步架构改动，且安全边界复杂）

---

## 3. 定位建议

### 3.1 DSL 的产品定位

EdgionDSL 的定位应是：**轻量级请求/响应规则引擎**，而非完整的通用脚本引擎。

具体来说：

- **核心场景**：Header 操作、IP 过滤、路径重写、条件 deny、简单认证逻辑
- **不应承担**：复杂业务逻辑、需要外部 IO 的场景、数据处理管道
- **与 WASM 的关系**（见第 6 节）

这个定位类似于：
- **Nginx `map` + `if` 块**：能做条件判断和变量替换，但不是 Lua
- **AWS WAF 条件规则**：结构化规则，覆盖 80% 网关场景
- **Cloudflare Workers Rules**：规则语言（不是 Workers JS）

核心价值主张：**零运行时依赖、编译时安全保证、亚毫秒级执行、可审计的字节码**。

### 3.2 与 APISIX Lua / Envoy Lua / Kong PDK 的对比

| 维度 | EdgionDSL（现状） | APISIX Lua | Envoy Lua | Kong PDK |
|------|------------------|------------|-----------|----------|
| 执行模型 | 同步 bytecode VM | LuaJIT（协程） | LuaJIT（协程） | LuaJIT + PDK |
| 读 Header | 已实现 | 完整 | 完整 | 完整 |
| 写 Header | 已实现 | 完整 | 完整 | 完整 |
| 读 Body | 未实现 | 支持 | 支持 | 支持 |
| 正则操作 | 部分（matches 可用） | 完整 | 完整 | 完整 |
| 调用 Redis | 未实现 | 支持（cosocket） | 不支持 | 支持（PDK） |
| HTTP 子请求 | 未实现 | 支持 | 支持 | 支持 |
| 安全隔离 | 强（字节码+沙箱） | 弱（共享 LuaVM） | 弱 | 弱 |
| 编译期检查 | 强（类型+函数签名） | 无 | 无 | 无 |
| 性能 | 亚毫秒（估算） | 微秒级（JIT） | 微秒级（JIT） | 微秒级（JIT） |

**结论**：EdgionDSL 在安全性和可预测性上优于 Lua，在灵活性上不及 Lua。应将差距集中在填补"基本用户需求"上，而非追求 Lua 的完整能力。

---

## 4. 增强优先级（按 ROI 排序）

### 优先级 P0：修复已声明但未实现的 API（必须先做）

| 序号 | 功能 | 理由 |
|------|------|------|
| P0-1 | 实现 `range()` 函数 | 语法已支持，运行时缺失，造成隐性 Bug |
| P0-2 | 实现 `sha256()` | 认证签名场景刚需，已声明 |
| P0-3 | 实现 `regex_find()` | Header 解析常用，已声明 |
| P0-4 | 实现 `regex_replace()` | URL 重写常用，已声明 |
| P0-5 | 修复或移除 `md5()` 声明 | md5 不安全，建议移除声明或标注 deprecated |

### 优先级 P1：高价值字符串操作

| 序号 | 功能 | 理由 |
|------|------|------|
| P1-1 | `trim(s)` | 处理 header 空白字符，极常用 |
| P1-2 | `split(s, delimiter)` 返回 List | 处理多值 header（Accept, X-Roles 等） |
| P1-3 | `s.replace(old, new)` 简单替换 | 路径/header 值替换 |
| P1-4 | 字符串 `%` 取模运算符 | 计算 hash 分桶，A/B 测试场景 |

### 优先级 P1：响应读取能力

| 序号 | 功能 | 理由 |
|------|------|------|
| P1-5 | `resp.status` 读取响应状态码 | 响应阶段条件处理的基础 |
| P1-6 | `resp.header("name")` 读取响应 header | 完善响应阶段读写对称性 |

### 优先级 P1：循环控制

| 序号 | 功能 | 理由 |
|------|------|------|
| P1-7 | `break` 语句 | 找到第一个匹配后退出，避免无效迭代 |
| P1-8 | `continue` 语句 | 跳过当前迭代 |

### 优先级 P2：中期增强

| 序号 | 功能 | 理由 |
|------|------|------|
| P2-1 | `return redirect(location, status)` | 重定向是网关核心功能之一 |
| P2-2 | `req.set_query("name", "value")` | 修改查询参数 |
| P2-3 | `req.remove_query("name")` | 删除查询参数 |
| P2-4 | `random_int(min, max)` | A/B 测试、随机采样 |
| P2-5 | 列表字面量 `["a", "b", "c"]` | 用户构造白名单/黑名单 |
| P2-6 | `in` 操作符（`x in list`） | 黑白名单检查 |
| P2-7 | `json_encode(map)` | 构造结构化 deny body |

### 优先级 P3：长期架构增强

| 序号 | 功能 | 理由 |
|------|------|------|
| P3-1 | 请求 Body 读取 | 需要异步架构支持 |
| P3-2 | 响应 Body 读取/修改 | 同上，且影响性能 |
| P3-3 | 用户自定义函数 | 代码复用 |
| P3-4 | Map 类型（用户构造） | 复杂数据结构 |
| P3-5 | 外部 HTTP 子请求 | 需要异步 + 安全沙箱重设计 |

---

## 5. 实现路径

### 5.1 P0：修复未实现函数

**改动位置**：`src/core/gateway/plugins/http/dsl/lang/builtins.rs`

所有 P0 函数只需在 `builtins.rs` 的 `call_builtin` match 分支中替换掉当前的 `Err(ApiError)` 桩即可，不需要改动 AST、编译器或字节码。

- `range(start, end)`：返回 `Value::List`（`start..end` 的字符串列表）
- `sha256(s)`：引入 `sha2` crate，计算 hex digest
- `regex_find(s, pattern)`：`pattern` 目前是运行时字符串参数，需要在运行时编译正则（有性能代价，建议限制 pattern 长度）
- `regex_replace(s, pattern, replacement)`：同上
- **难度**：低，单文件改动，无架构影响

### 5.2 P1：字符串操作扩展

**改动位置**：
1. `lang/ast.rs`：无需改动（通过 FnCall 路由）
2. `lang/compiler.rs`：在 `compile_fn_call` 和 `compile_method_call` 中注册新函数
3. `lang/bytecode.rs`：`BuiltinId` 添加新变体（`Trim`, `Split`, `Replace` 等）
4. `lang/builtins.rs`：实现分发逻辑

`split()` 特殊处理：返回 `Value::List(Vec<String>)`，当前 `ListGet` 和 `ListLen` 指令已存在，可以直接用 `for-in` 迭代。

- **难度**：中等，多文件联动，但模式清晰

### 5.3 P1：响应读取（`resp.status` / `resp.header`）

**改动位置**：
1. `PluginSession` trait（`session.rs`）：确认是否有 `get_response_status()` 方法（当前有 `set_response_status()`，但没有 get）
2. `lang/bytecode.rs`：添加 `BuiltinId::RespStatus`, `BuiltinId::RespHeader`
3. `lang/compiler.rs`：注册 `resp.status`（0 参）、`resp.header("name")`（1 参）
4. `lang/builtins.rs`：实现读取逻辑

**注意**：`resp.*` API 只在响应阶段有意义，但目前 `DslPlugin` 只实现了 `RequestFilter`。若要在响应阶段运行脚本，需要额外实现 `UpstreamResponseFilter` trait 并增加 `phase: request | response` 配置。这是一个中等规模的架构改动。

- **难度**：中等（如果只做读 header），高（如果要支持响应阶段执行）

### 5.4 P1：`break` / `continue`

**改动位置**：
1. `lang/ast.rs`：添加 `StmtKind::Break`、`StmtKind::Continue`
2. `lang/parser.rs`：解析 `break` / `continue` 关键字
3. `lang/bytecode.rs`：添加 `OpCode::BreakLoop` / `OpCode::ContinueLoop`（或复用 Jump + 回填）
4. `lang/compiler.rs`：编译器需要维护 `loop_exit_patches`（break 回填列表）和 `loop_continue_target`（continue 跳转目标）

- **难度**：中等，编译器需要维护额外的跳转回填状态

### 5.5 P2：`return redirect(location, status)`

**改动位置**：
1. `types/filters.rs`：`PluginRunningResult` 添加 `Redirect { status, location }` 变体（或复用 `ErrResponse` 并在 plugin 层处理）
2. `lang/ast.rs`：`StmtKind` 添加 `ReturnRedirect { location, status }`
3. `lang/parser.rs`：解析 `return redirect(url)` / `return redirect(url, 301)`
4. `lang/compiler.rs` / `lang/bytecode.rs`：添加 `OpCode::ReturnRedirect`
5. `lang/vm.rs`：处理 ReturnRedirect 并返回对应 PluginRunningResult

- **难度**：中等，多文件，但逻辑简单

### 5.6 P2：列表字面量 + `in` 操作符

**改动位置**：
1. `lang/ast.rs`：`ExprKind::ListLit(Vec<Expr>)`
2. `lang/parser.rs`：解析 `["a", "b", "c"]`
3. `lang/bytecode.rs`：添加 `OpCode::MakeList(u16)`（从栈上弹出 n 个元素构造 List）
4. `lang/compiler.rs`：编译列表字面量
5. `lang/vm.rs`：实现 `MakeList`

`in` 操作符额外需要：
1. `BinOp::In` 变体
2. `OpCode::InList` 指令（pop value + list, push bool）

- **难度**：中等

---

## 6. MVP 定义

MVP（最小可用版本）的目标是：**DSL 能覆盖 80% 的网关脚本场景，使用者不会遇到"声明了但不能用"的尴尬**。

### MVP 功能范围

**必须完成**（阻塞 MVP）：
- [ ] P0-1：`range()` 实现（修复隐性 Bug）
- [ ] P0-2：`sha256()` 实现
- [ ] P0-3：`regex_find()` 实现
- [ ] P0-4：`regex_replace()` 实现
- [ ] P0-5：移除或标注 `md5()` 为废弃

**应该完成**（MVP 标配）：
- [ ] P1-1：`trim(s)`
- [ ] P1-2：`split(s, delimiter)`
- [ ] P1-3：字符串 `.replace(old, new)` 方法
- [ ] P1-7：`break`
- [ ] P1-8：`continue`

**有助于 MVP 完整性**（可选）：
- [ ] P2-1：`return redirect(location, status)`
- [ ] P2-5：列表字面量 `["a", "b", "c"]`
- [ ] P2-6：`in` 操作符

### MVP 能覆盖的场景示例

```dsl
// 场景1：IP 黑名单
let ip = req.client_ip
let blocked = ["1.2.3.4", "5.6.7.8"]
if ip in blocked {
    return deny(403, "IP blocked")
}

// 场景2：JWT Header 格式校验
let auth = req.header("Authorization")
if auth == nil || !auth.starts_with("Bearer ") {
    return deny(401, "Missing token")
}
let token = substr(auth, 7, len(auth))
let sig = sha256(token + "secret")
req.set_header("X-Token-Sig", sig)

// 场景3：路径改写 + 参数传递
let uid = req.path_param("uid")
if uid == nil {
    return deny(400, "Missing uid")
}
req.set_uri("/internal/users/" + uid)
req.set_header("X-User-ID", uid)

// 场景4：多值 Header 处理
let roles = req.header("X-Roles")
if roles != nil {
    let parts = split(roles, ",")
    for role in parts {
        let trimmed = trim(role)
        if trimmed == "admin" {
            req.set_header("X-Is-Admin", "true")
            break
        }
    }
}
```

---

## 7. 与 WASM 插件的关系

### 7.1 两者定位分工

| 维度 | EdgionDSL | WASM 插件 |
|------|-----------|-----------|
| 目标用户 | 运维/DevOps，YAML 内联脚本 | 开发者，复杂插件开发 |
| 复杂度上限 | 低（条件判断、header 操作） | 高（任意逻辑） |
| 部署方式 | YAML 内联，无需构建 | 需要编译 wasm 文件 |
| 性能 | 极轻量（~微秒） | 轻量（有 JIT/AOT） |
| 安全隔离 | 内置沙箱 | WASM 沙箱（更强） |
| IO 能力 | 受限（无异步 IO） | 视接口开放程度 |
| 可审计性 | 高（字节码可反汇编） | 中（需要工具链） |

### 7.2 建议

1. DSL 和 WASM **不冲突**，应并行推进，面向不同复杂度的需求
2. DSL 不应追求"完整通用语言"，当需求超出 DSL 边界时，引导用户使用 WASM
3. DSL 的价值在于**零运行时依赖、编译期安全保证**，不应为了追求 WASM 的能力而引入不安全的 API（如无限制的外部 HTTP 调用）
4. 未来 WASM 插件成熟后，DSL 可以退化为"快速规则引擎"层，WASM 处理复杂场景

---

## 8. 下一步：第一个 PR 做什么

**PR-1：修复未实现的内置函数（纯 builtins.rs 改动）**

范围：
1. 实现 `range(start, end)` → 返回 `Value::List`（字符串列表，`["0","1","2"...]`）
2. 实现 `sha256(s)` → 引入 `sha2` crate，返回 hex 字符串
3. 实现 `regex_find(s, pattern)` → 返回第一个捕获组（无捕获组时返回完整匹配）
4. 实现 `regex_replace(s, pattern, replacement)` → 替换所有匹配

**不改动**：AST、parser、bytecode（BuiltinId 已存在）

**预估工作量**：0.5 天

**PR-2：字符串操作扩展（trim / split / replace）**

范围：
1. `trim(s)` / `trim_start(s)` / `trim_end(s)`
2. `split(s, delimiter)` 返回 List
3. `s.replace(old, new)`

**需要改动**：bytecode.rs（3 个新 BuiltinId）、compiler.rs（注册函数）、builtins.rs（实现）

**预估工作量**：1 天

**PR-3：break / continue**

**预估工作量**：1 天（需要仔细处理编译器的跳转回填）

**PR-4：列表字面量 + `in` 操作符**

**预估工作量**：1.5 天

完成以上 4 个 PR 后，DSL 达到 MVP 标准，可以面向用户开放。

---

## 附录：现有 Bug / 设计不一致点

1. **`range()` 声明与实现不一致**：BuiltinId::Range 存在，但 builtins.rs 中返回 unimplemented，且 `for i in range(0, 5)` 实际走的是 `ForRange` AST 路径，而不是 `FnCall("range")` 路径——两者实现机制不同，容易混淆
2. **空字符串是 truthy**：`if req.header("X-Foo")` 中，即使 header 存在但值为空字符串，条件也为真——这与大多数用户预期不符
3. **整数 0 是 truthy**：`if 0` 为 true，这与 Python/JS 相反
4. **`md5()` 不应在 gateway 中提供**：md5 已被废弃用于安全场景，公开此 API 可能误导用户
5. **`regex_find()` 和 `regex_replace()` 的 pattern 参数是运行时字符串**：无法像 `matches()` 那样在编译期预编译正则，存在运行时正则编译开销，需要加 LRU 缓存
6. **`req.scheme` 依赖代理 header**：从 X-Forwarded-Proto 读取 scheme，但这个 header 可以被客户端伪造，文档中需要说明
