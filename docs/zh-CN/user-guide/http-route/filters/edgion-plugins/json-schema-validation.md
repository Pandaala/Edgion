# JSON Schema 校验插件

> **🔌 Edgion 扩展**
>
> `JsonSchemaValidation` 是由 `EdgionPlugins` CRD 提供的请求插件，不属于标准 Gateway API。

## 概述

`JsonSchemaValidation` 在请求到达后端前，使用内联 JSON Schema 校验 JSON 请求体。插件采用 JSON Schema Draft 2020-12，拒绝格式错误的 JSON 和重复对象键，并且不会重新编码有效请求体，原始字节会原样转发。

该插件只能用于请求阶段，并固定采用失败关闭策略。每个 `requestPlugins` 列表最多包含一个 `JsonSchemaValidation` 条目；如需组合多个约束，请在一个 Schema 内使用 `allOf` 等关键字。

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: validate-create-user
spec:
  requestPlugins:
    - type: JsonSchemaValidation
      config:
        schema:
          $schema: https://json-schema.org/draft/2020-12/schema
          type: object
          required: [name, email]
          properties:
            name:
              type: string
              minLength: 1
            email:
              type: string
              format: email
          additionalProperties: false
```

通过 `HTTPRoute` 的 `ExtensionRef` 过滤器引用此资源。详见[插件组合与引用](../plugin-composition.md)。

## 配置

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `config.schema` | JSON 值 | 是 | 内联 Draft 2020-12 Schema |
| `body.maxBodySize` | 字节大小 | 否 | 条目级请求体限制，默认 `64KiB`，允许的最大值为 `1MiB` |
| `conditions` | 通用插件条件 | 否 | 条件不匹配时跳过整个插件，包括请求体捕获 |

不支持 `body.conditions`，因为请求体校验不能被配置成条件式失败开放。条目级 `alias` 和 `dye` 继续使用通用插件条目契约。

内联 Schema 采用固定安全限制，包括 64 KiB 序列化大小、64 层深度、4,096 个 Schema 节点、64 个 pattern，以及有界正则表达式编译。超过限制的配置会被拒绝。

## 支持的 Schema 功能

方言为 Draft 2020-12；安全且受支持的内置 `format` 会执行断言，未知 format 保持注解语义。由于 `format: regex` 会在请求路径编译由请求内容控制的正则表达式，因此该格式会在配置阶段被拒绝。

仅支持指向根文档内部的本地 JSON Pointer 引用：

```yaml
config:
  schema:
    type: object
    properties:
      address:
        $ref: "#/$defs/address"
    $defs:
      address:
        type: object
        required: [city]
        properties:
          city:
            type: string
```

外部引用和外部资源读取始终禁用。`$id`、`$anchor`、`$dynamicAnchor`、`$dynamicRef`、`$recursiveRef`、`unevaluatedProperties` 以及本地引用环都会被拒绝。Schema 必须完整内联且不存在引用环。

## 请求要求

对于非空请求体，插件接受：

- `Content-Type: application/json`；
- `application/*+json` 子类型，例如 `application/problem+json`；
- 不带 `charset`，或大小写不敏感的 `charset=utf-8`；
- 其他语法正确的媒体类型参数；
- 不带 `Content-Encoding`，或使用 `Content-Encoding: identity`。

缺失、语法错误、重复或逗号合并的 `Content-Type` 会被拒绝。非 JSON 媒体类型、非 UTF-8 字符集，以及 gzip 等编码请求体同样会被拒绝。

空请求体始终按无效 JSON 处理并返回 `400`，即使它没有 `Content-Type`。

## 处理结果

| 条件 | HTTP 状态码 | 响应体 |
|------|-------------|--------|
| 有效 JSON 且符合 Schema | 继续转发到后端 | 不适用 |
| 空、格式错误、重复键、超出工作预算或不符合 Schema 的 JSON | `400` | `Invalid request body` |
| 不支持的媒体类型、字符集或内容编码 | `415` | `Unsupported media type` |
| 超过逻辑或物理请求体限制 | `413` | 空 |
| 请求体读取超时 | `408` | 空 |
| 请求体捕获、存储、校验器或运行时不可用 | `500` | 空 |

校验详情、实例值、Schema 内容、存储错误和 panic 详情都不会返回给客户端，也不会写入插件日志。

## 示例

### 限制事件信封结构

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: validate-event
spec:
  requestPlugins:
    - type: JsonSchemaValidation
      body:
        maxBodySize: 256KiB
      config:
        schema:
          type: object
          required: [eventType, payload]
          properties:
            eventType:
              enum: [created, updated]
            payload:
              type: object
          additionalProperties: false
```

### 校验有大小上限的数组

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: validate-batch
spec:
  requestPlugins:
    - type: JsonSchemaValidation
      config:
        schema:
          type: array
          minItems: 1
          maxItems: 100
          items:
            type: object
            required: [id]
            properties:
              id:
                type: string
            additionalProperties: false
```

### 接受两种结构之一

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: validate-command
spec:
  requestPlugins:
    - type: JsonSchemaValidation
      config:
        schema:
          oneOf:
            - type: object
              required: [start]
              properties:
                start:
                  type: boolean
              additionalProperties: false
            - type: object
              required: [stop]
              properties:
                stop:
                  type: boolean
              additionalProperties: false
```

## 运行行为

Schema 只会在配置校验和运行时发布阶段编译，不会在请求路径编译。无效的初始配置会报告 `Accepted=False`，并以固定 `500` 响应关闭流量。无效热更新也会用失败关闭运行时替换旧运行时，而不是保留最后一次有效配置；之后应用有效 Schema 可恢复正常流量。

请求体使用请求范围内的共享存储。按照网关请求体策略，请求体可能落盘，但通过校验的原始字节会原样重放到后端。

## 安全与性能

- 如果不允许未知输入字段，请使用 `additionalProperties: false`。
- 将 `body.maxBodySize` 设置为 API 实际需要的最小值；默认值为 `64KiB`。
- 不要在 Schema 注解中放置密钥或内部实现细节。
- 优先使用简单且带锚点的 pattern；不支持的回溯专用正则特性会在配置阶段被拒绝。
- 插件会为每个大小合规的请求构建 JSON 值。无需结构化请求体校验的路由不要启用该插件。

## 常见问题

### 能否从 URL 或文件加载 Schema？

不能。Schema 必须内联，并且外部资源读取已禁用。

### 插件会修改有效 JSON 吗？

不会。校验过程使用解析后的表示，但转发的是原始存储字节。

### 能否自定义错误响应？

不能。首个版本固定使用状态码和通用响应体，以避免泄露校验细节。

### 能否校验压缩后的 JSON？

不能。请发送 identity 编码的 JSON；其他 `Content-Encoding` 都会返回 `415`。
