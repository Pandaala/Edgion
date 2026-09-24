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

## 提交前检查规则支持情况

对单个 `EdgionPlugins` YAML 文件运行本地命令：

```sh
edgion-cli waf inspect --file application-waf.yaml --format json
```

此命令无需连接 Controller，也无需令牌。输入上限为 16 MiB，输出为带版本号的报告。
`analysisComplete` 只表示 SecLang 源码完成了兼容性分析，**不表示可以发布**；
只有 `strictValidationPassed: true` 才表示该 WAF 配置通过与 Controller、Gateway
相同的严格编译及请求体检查。未通过时命令以非零状态退出。

例如，需要聚合完整上游响应体的规则会产生 `full_response_body` 条目，包含规则 ID、
从零开始的文档和指令序号，以及移除该规则的建议。缺少 ID 的规则会使分析停止，
报告安全的物理行号并提示添加 ID，不会捏造编号。`unattributedFindings` 统计无法归属规则 ID 的条目，因此
`unavailableRuleCount` 不宣称包含这些条目。语法、缺失资源、不支持语义和资源限额错误以固定 `category` 表示，
可定位时还会提供安全的位置。报告不会包含规则原文、正则、操作数或文件身份。
`omittedFindings` 和 `omittedRuleSummaries` 标出超过 128 项展示上限的条目。
分析未完成时没有兼容性计数；先修正报告中的失败，再重新检查。

`edgion-resources` 的公开 `WafConfig::inspect_support()` 返回同一份无原始值的报告，
EdgionCenter 等工具可以复用它，无需另写 SecLang 解析器；调用方仍需自行限制输入字节数
并控制报告访问权限。

## SQL 注入和 XSS 操作符

`@detectSQLi` 和 `@detectXSS` 是无参数 SecLang 操作符。Edgion 用原生 Rust 实现
libinjection-go v0.3.2 的检测行为，对规则选中的每个值先执行已配置的变换，再进行检测。

```yaml
requestBody:
  inspection: full
rules: |
  SecRule ARGS_GET "@detectSQLi" "id:1101,phase:1,deny,status:403,t:urlDecodeUni,capture"
  SecRule ARGS_GET "@detectXSS" "id:1102,phase:1,deny,status:403,t:urlDecodeUni"
```

SQL 注入规则设置 `capture` 且正向命中时，指纹写入 `TX.0`，供本规则动作和后续规则使用。
XSS 检测不会新增捕获，也不会覆盖已有的 `TX.0`。取反匹配不捕获；链式规则失败时，
命中结果和捕获均不提交。目标选择、变换和链式行为沿用 WAF 事务的既有语义。

检测器会读取所选值的完整内容；若规则使用 `REQUEST_BODY` 或从正文解析出的参数，
应设置 `requestBody.inspection: full`。使用 `capture` 时，包括仅检查请求头的规则，
前缀模式也要求切换为 `full`。每个值的检测输入上限为 4 MiB，多个 SQL
解析上下文或 HTML 上下文的工作量计入请求的有界执行预算。超限按 WAF 既有运行时错误
策略失败关闭。此实现不使用 Hyperscan。

## 请求体检查

### 请求级控制动作

设置 `requestBody.inspection: full` 后，严格编译器支持四种 `ctl` 动作：
`forceRequestBodyVariable=On|Off`、
`requestBodyProcessor=URLENCODED|MULTIPART|JSON|XML`、
`ruleRemoveByTag=<tag>`、`ruleRemoveTargetByTag=<tag>;<target>`。
动作仅修改当前请求的 WAF 事务。审计和引擎控制等其他 `ctl` 动作仍会导致整个规则集被拒绝；
前缀检查模式不接受任何 `ctl` 动作。

请求体控制动作必须在阶段 1 执行，并先于请求体解析。处理器动作覆盖当前请求的请求体解析器；
`forceRequestBodyVariable=On` 在结构化解析结果之外暴露原始 `REQUEST_BODY`，
`Off` 则隐藏原始值。这些动作不会修改配置的检查模式或请求体物理上限。
按标签排除规则或目标的动作从控制规则命中后开始生效，先前执行的规则不受影响；
目标排除仅作用于后续带相应标签的规则及其链成员中的指定集合和目标。
失败的规则链不会提交控制动作。控制条目数和工作量均有限额，超限按 WAF 运行时错误策略处理。

```yaml
requestBody:
  inspection: full
rules: |
  SecRule REQUEST_HEADERS:X-Waf-Exempt "@streq yes" "id:1201,phase:1,pass,ctl:ruleRemoveByTag=managed-check"
  SecRule ARGS "@contains blocked" "id:1202,phase:2,deny,status:403,tag:managed-check"
```

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
