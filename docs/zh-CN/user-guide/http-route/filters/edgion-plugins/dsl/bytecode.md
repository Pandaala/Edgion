# 字节码模式

## 什么是字节码模式

DSL 插件支持两种加载方式：`source`（源码）和 `bytecode`（预编译字节码）。

**源码模式**：Controller 检查配置时会编译源码并进行静态校验，但不会用字节码替换序列化的 `source` 字段。Gateway 收到该配置后，在构建插件时独立编译并校验源码。

**字节码模式**：在配置中提供预编译字节码。Controller 校验和 Gateway 构建都会解码、检查版本并进行静态校验。字节码模式跳过源码解析和编译，不会跳过校验或 Gateway 的 VM 初始化。

`source` 和 `bytecode` 至少提供一个，也可以同时提供；此时 `bytecode` 优先，即使字节码无效也不会回退到源码。两个已配置字段都会保留在序列化配置中。编译和 VM 构建发生在配置处理期间，不会逐请求执行。

## 字节码格式

字节码的传输格式为 Base64 编码的 JSON 字符串。

### JSON 结构

```json
{
  "version": 4,
  "code": [...],
  "constants": [...],
  "local_count": 16,
  "max_loop_depth": 2,
  "source": "..."
}
```

各字段说明：

| 字段 | 类型 | 说明 |
|------|------|------|
| `version` | 整数 | 字节码格式版本号，当前版本为 `4` |
| `code` | 数组 | 指令序列（`Vec<OpCode>`），VM 逐条执行 |
| `constants` | 数组 | 常量池（`Vec<Constant>`），指令通过索引引用 |
| `local_count` | 整数 | 局部变量槽数 |
| `max_loop_depth` | 整数 | 最大循环嵌套深度 |
| `source` | 字符串 | 可选，原始源码，仅用于调试和日志输出 |

### 常量类型

常量池中的每个条目属于以下类型之一：

- `Str`：字符串常量
- `Int`：整数常量
- `Regex`：供字符串查找/替换操作使用的正则表达式
- `StreamingRegex`：供增量 `.matches()` 使用的前向 DFA 模式
- `JsonPath`：供 `.json_get()` 使用的已校验确定性 JSONPath
- `JsonSelect`：供 `.json_select()` 使用的静态命名多路径计划

## 何时使用字节码

| 场景 | 推荐模式 | 原因 |
|------|----------|------|
| 开发调试 | `source` | 可读性好，修改方便，无需预编译步骤 |
| 高频路由 | `bytecode` | 跳过编译开销，Gateway 启动或重载更快 |
| CI/CD 流水线 | `bytecode` | 编译在构建阶段完成，部署时直接使用，确保一致性 |
| 跨版本分发 | `source` 或重新构建的 `bytecode` | 字节码版本严格匹配，必须针对目标 Edgion 版本重新构建 |

## 大小限制

- 解码后最大 **256 KB**（对应常量 `MAX_DECODED_BYTECODE_LEN`）
- Base64 编码后约 **341 KB**（Base64 膨胀系数约 4/3）
- 超出限制时，Controller 校验报告错误，Gateway 的独立校验也会拒绝无效制品

## 版本兼容性

字节码携带版本号（`version` 字段），反序列化时严格校验。

- 版本号不匹配时，Controller 校验报告版本错误，Gateway 校验也会在 VM 初始化前独立拒绝该字节码
- 升级 Edgion 后如果字节码格式版本发生变更，已有字节码需要重新编译
- 建议在 CI/CD 中将字节码与 Edgion 版本绑定，避免版本漂移

当前版本号：`BYTECODE_VERSION = 4`

版本 4 新增显式的原始 Body MD5 和 HMAC-SHA-256 指令。版本 3 新增静态有界的
`JsonSelect` 常量和指令，但现在版本 3 以及所有更早版本和无版本字节码都会被
拒绝，必须使用目标 Edgion 版本从源码重新编译。

## 安全校验

完成有界解码和版本检查后，两个进程中的共享校验器执行以下静态校验，任一项不通过即拒绝加载：

1. **版本号匹配**：`version` 字段与运行时期望版本一致
2. **常量索引合法**：所有 `LoadConst` 指令引用的常量索引在常量池范围内
3. **指令与常量类型匹配**：值加载仅接受 `Str`/`Int`；`.matches()`、字符串正则操作、`.json_get()` 和 `.json_select()` 分别要求 `StreamingRegex`、`Regex`、`JsonPath` 和 `JsonSelect` 常量
4. **正则重建校验**：解码后由校验器在自动机大小受限的条件下重新编译正则，验证当前运行环境中的语法和资源限制
5. **JSONPath 校验**：重新解析每个 `JsonPath` 常量，并要求其仍为确定性路径
6. **JSON 多路径校验**：每个 selector 必须非空且有界，输出名必须唯一，路径必须为确定性路径

## 配置示例

### 使用源码（开发环境）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-check
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "auth-check"
        source: |
          let token = req.header("Authorization")
          if token == nil {
            return deny(401, "Unauthorized")
          }
```

### 使用字节码（生产环境）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-check
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "auth-check"
        bytecode: "BASE64_ENCODED_BYTECODE_V4"
```

### 同时提供源码和字节码

同时配置时，`bytecode` 优先生效；配置中的 `source` 仍参与序列化，但不会执行。字节码内部可以另带编译时嵌入的源码元数据；加载时不会把配置的 `source` 复制到该元数据中。

```yaml
config:
  name: "auth-check"
  source: |
    let token = req.header("Authorization")
    if token == nil {
      return deny(401, "Unauthorized")
    }
  bytecode: "BASE64_ENCODED_BYTECODE_V4"
```

## 相关文档

- [DSL 概述](./README.md)
- [沙盒安全模型](./sandbox-security.md)
- [语言语法参考](./language-syntax.md)
