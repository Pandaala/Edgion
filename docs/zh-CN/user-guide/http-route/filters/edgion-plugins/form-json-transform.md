# Form/JSON 转换插件

> **🔌 Edgion 扩展**
>
> `FormJsonTransform` 是由 `EdgionPlugins` CRD 提供的请求插件。

## 概述

`FormJsonTransform` 在 `application/x-www-form-urlencoded` 与 JSON 之间转换有界请求体，适合对接扁平的旧式表单接口。转换方向必须显式配置，不会根据客户端请求头自动推断。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: form-to-json
spec:
  requestPlugins:
    - type: FormJsonTransform
      config:
        direction: formToJson
```

| 字段 | 必填 | 取值 |
|------|------|------|
| `config.direction` | 是 | `formToJson`、`jsonToForm` |
| `body.maxBodySize` | 否 | 默认 `64KiB`，最大 `1MiB`；同一个有效限制同时约束输入和输出 |

插件只能用于请求阶段，并固定失败关闭。配置 `body.conditions` 或 `body.onReadFailure: failOpen` 会被拒绝。条目级 `conditions` 仍可在读取请求体前跳过整个插件。

## 映射规则

Form 转 JSON：

- 百分号转义必须是完整的十六进制字节，解码结果必须是有效 UTF-8；
- `+` 解码为空格；
- 单次出现的键转换为 JSON 字符串；
- 重复键转换为字符串数组，并保留同一键的值顺序；
- 不推断数字、布尔值、null、方括号或嵌套结构；
- 忽略空的 `&` 分段，但保留空键和空值；
- 空请求体转换为 `{}`。

JSON 转 Form 时，根节点必须是对象。成员值可为字符串、数字、布尔值，或由这些标量构成的非空数组。null、对象、嵌套数组和空数组会被拒绝。空对象转换为空请求体。

键按字典序输出，数组值保持原顺序。Form 输出遵循 WHATWG 编码规则：ASCII 字母数字以及 `*`、`-`、`.`、`_` 原样保留，空格变为 `+`，其他 UTF-8 字节使用大写 `%HH`。JSON 数字使用网关 JSON 解析器的规范表示，不保留原始数字字面量。

该映射不是字节级往返转换：JSON 对象无法表示不同键之间的 Form 元组顺序；Form 输入始终是字符串类型；单个值会变为标量，而不是单元素数组。

## 限制与请求要求

每次转换最多处理 256 个键值对；解码后的键最多 256 字节；标量值最多 16 KiB。JSON 同时使用共享的深度和节点数限制。任何限制都在发布新请求体之前执行。

`formToJson` 要求恰好一个 `Content-Type: application/x-www-form-urlencoded`，唯一允许的可选参数是 `charset=utf-8`。`jsonToForm` 接受 `application/json` 或具体的 `application/*+json` 类型，并要求 UTF-8。两个方向都只接受缺省 `Content-Encoding`，或单个 `identity`。

转换成功后，`Content-Type` 会被规范化为 `application/json` 或 `application/x-www-form-urlencoded`。上游请求 framing 根据新请求体重新计算。已接受的 `Content-Encoding: identity` 会被保留。

| 条件 | HTTP 状态码 | 响应体 |
|------|-------------|--------|
| 转换成功 | 继续请求后端 | 不适用 |
| 编码或 JSON 错误，或者 JSON 结构不受支持 | `400` | `Invalid request body` |
| 媒体类型、字符集或内容编码不受支持 | `415` | `Unsupported media type` |
| 请求体、输出、键值对、键或值超限 | `413` | 空 |
| 请求体存储或修改失败 | `500` | 空 |

## 插件组合

后续插件会看到前序插件已发布的请求体和媒体类型。因此，`formToJson` 后可继续执行 `JsonSchemaValidation` 或 `ProxyRewrite` 的 JSON Body 操作。后序转换插件会用自己的限制检查已经转换的请求体。同方向连续转换时，第二个插件会因媒体类型已经改变而返回 `415`。

请求体复用共享的请求级 Body 存储，包括磁盘 spill 和重试重放；插件不会缓存响应体。
