# 字节码模式

## 什么是字节码模式

DSL 插件支持两种加载方式：`source`（源码）和 `bytecode`（预编译字节码）。

**源码模式**：Controller 收到配置后，在线将源码编译为字节码，再通过 gRPC 推送给 Gateway。每次配置变更或 Controller 重启时均会重新编译。

**字节码模式**：直接在配置中提供已编译好的字节码，Controller 跳过编译步骤，直接将字节码推送给 Gateway。

当配置中同时提供 `source` 和 `bytecode` 时，优先使用 `bytecode`，`source` 字段会被忽略。

## 字节码格式

字节码的传输格式为 Base64 编码的 JSON 字符串。

### JSON 结构

```json
{
  "version": 1,
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
| `version` | 整数 | 字节码格式版本号，当前版本为 `1` |
| `code` | 数组 | 指令序列（`Vec<OpCode>`），VM 逐条执行 |
| `constants` | 数组 | 常量池（`Vec<Constant>`），指令通过索引引用 |
| `local_count` | 整数 | 局部变量槽数 |
| `max_loop_depth` | 整数 | 最大循环嵌套深度 |
| `source` | 字符串 | 可选，原始源码，仅用于调试和日志输出 |

### 常量类型

常量池中的每个条目属于以下类型之一：

- `Str`：字符串常量
- `Int`：整数常量
- `Regex`：预编译的正则表达式（在字节码序列化时已完成编译）

## 何时使用字节码

| 场景 | 推荐模式 | 原因 |
|------|----------|------|
| 开发调试 | `source` | 可读性好，修改方便，无需预编译步骤 |
| 高频路由 | `bytecode` | 跳过编译开销，Gateway 启动或重载更快 |
| CI/CD 流水线 | `bytecode` | 编译在构建阶段完成，部署时直接使用，确保一致性 |
| 跨版本分发 | `bytecode` | 字节码带版本号，可安全传输和长期存储 |

## 大小限制

- 解码后最大 **256 KB**（对应常量 `MAX_DECODED_BYTECODE_LEN`）
- Base64 编码后约 **341 KB**（Base64 膨胀系数约 4/3）
- 超出限制时，Controller 拒绝该配置并返回校验错误

## 版本兼容性

字节码携带版本号（`version` 字段），反序列化时严格校验。

- 版本号不匹配时，Controller 拒绝加载并在日志中报告版本冲突
- 升级 Edgion 后如果字节码格式版本发生变更，已有字节码需要重新编译
- 建议在 CI/CD 中将字节码与 Edgion 版本绑定，避免版本漂移

当前版本号：`BYTECODE_VERSION = 1`

## 安全校验

反序列化阶段执行以下静态校验，任一项不通过即拒绝加载：

1. **版本号匹配**：`version` 字段与运行时期望版本一致
2. **常量索引合法**：所有 `LoadConst` 指令引用的常量索引在常量池范围内
3. **正则常量类型正确**：所有正则相关指令（`Match`、`NotMatch` 等）引用的常量类型必须为 `Regex`
4. **正则重新编译验证**：反序列化时对每个 `Regex` 常量重新编译，确保正则表达式在当前运行环境下有效

## 配置示例

### 使用源码（开发环境）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-check
spec:
  requestPlugins:
      type: Dsl
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
      type: Dsl
      config:
        name: "auth-check"
        bytecode: "eyJ2ZXJzaW9uIjoxLCJjb2RlIjpbLi4uXSwi..."
```

### 同时提供源码和字节码

当同时配置 `source` 和 `bytecode` 时，`bytecode` 优先生效，`source` 仅作为调试参考保留在字节码的 `source` 字段中（如果编译时已嵌入）。

```yaml
config:
  name: "auth-check"
  source: |
    let token = req.header("Authorization")
    if token == nil {
      return deny(401, "Unauthorized")
    }
  bytecode: "eyJ2ZXJzaW9uIjoxLCJjb2RlIjpbLi4uXSwi..."
```

## 相关文档

- [DSL 概述](./README.md)
- [沙盒安全模型](./sandbox-security.md)
- [语言语法参考](./language-syntax.md)
