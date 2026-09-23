# 原生 WAF

Edgion 使用 Rust 实现有界 SecLang 编译和 WAF 事务执行。WAF 配置位于
`EdgionPlugins.spec.waf`，通过 HTTPRoute 的 ExtensionRef 引用。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: application-waf
  namespace: default
spec:
  waf:
    mode: on
    priority: 100
    requestBody:
      inspection: prefix
      prefixSize: 64KiB
    rules: |
      SecRule REQUEST_URI "@contains /admin" "id:1001,phase:1,deny,status:403"
      SecRule REQUEST_BODY "@contains forbidden" "id:1002,phase:2,deny,status:406"
```

## 配置

| 字段 | 必填 | 默认值 | 含义 |
|------|------|--------|------|
| `rules` | 是 | — | 编译为不可变规则版本的内联 SecLang 源码 |
| `mode` | 否 | `on` | `on` 执行规则阻断；`detectionOnly` 仅记录规则命中 |
| `priority` | 否 | `0` | 0–65535，较大值优先；相同时 Route、Gateway、全局依次优先 |
| `requestBody.inspection` | 否 | `prefix` | `prefix` 提前检查指定窗口；`full` 检查完整有界请求体 |
| `requestBody.prefixSize` | 否 | `64KiB` | 前缀窗口，1 字节至 `1MiB`，支持 `16KiB`、`100k`、`1m` 等大小字符串；仅用于 `prefix` |

规则源码不会写入错误响应、日志或指标，网关管理接口将其脱敏。

## 请求体检查

Phase 1 在读取正文前检查请求头等信息。默认的 `prefix` 模式在选择或连接后端前，
检查最多 `prefixSize` 字节正文，默认 64 KiB、最大 1 MiB；移除传输分帧，但不解压 `Content-Encoding`。
规则阻断时，后端收到零请求头、零正文。通过只表示检查窗口未被规则阻断，
窗口之后的内容不会继续接受 WAF 正文检查。

前缀模式的正文匹配仅接受保守的正向原始 `REQUEST_BODY` 的 `@contains`、`@pm` 等已支持组合。
Phase 2 中仅检查头部、URI 或查询参数集合（`ARGS_GET`、`ARGS_GET_NAMES`）的规则正常求值，
不读取正文；整个规则版本都不消费正文时，`prefix` 和 `full` 都不缓存正文。
混合头部与正文规则保持配置顺序。正文否定匹配、结构化正文、链式、控制流和完整正文依赖必须显式使用 `full`。
校验覆盖整个规则版本，包括后续阶段的正文依赖。

只有 WAF 需要正文时，共享请求体存储最多缓存检查窗口，包括 1 MiB 窗口在内均使用内存。
前缀缓存的内存上限和总量上限均为窗口大小，不受完整正文落盘阈值影响。通过后先原样发送
缓存，再接续剩余正文，字节不丢失、不重复。已经存在完整缓存时直接复用。
前缀检查后的正文修改及不兼容流式处理器会失败关闭。原生前缀捕获禁用上游重试，
短正文也相同；复用完整缓存时沿用已有重试策略。

规则消费正文时，`full` 在 phase 2 前等待完整有界捕获，复用现有内存／磁盘存储；原始字节规则也按此执行。
全量模式不配置 `prefixSize`。网关物理正文上限和 WAF 执行预算始终独立生效。前缀放行后才发现超限时，后端可能已收到数据。
`detectionOnly` 不会忽略传输、存储或物理上限错误。

## 响应及热更新

Phase 3 在响应头发送给客户端前执行。主上游响应始终流式处理，配置不允许完整
`RESPONSE_BODY` 聚合；phase 4 使用有界响应语义，phase 5 对正常完成的事务执行一次。
日志只记录有界规则 ID 和计数，不记录命中值或正文。

新配置先完整编译再原子发布；进行中的请求保留原规则版本。无效更新报告
`Accepted=False` 并保留最近可用版本。删除资源后，新请求不能继续解析该资源。
后端级 WAF 配置不受支持，因为 WAF 请求阶段先于后端选择。

编译器严格拒绝未实现的指令、操作符、动作、外部资源及完整响应体要求，
不能据此宣称兼容所有 ModSecurity 或 CRS 配置。
