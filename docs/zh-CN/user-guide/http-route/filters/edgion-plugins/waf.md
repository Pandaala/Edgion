# 原生 WAF

Edgion 使用 Rust 实现有界 SecLang 编译和 WAF 事务执行。规则放在有类型的
`WafRuleBundle` EdgionConfigData 中；`WafPolicy` EdgionConfigData 聚合并选择规则包；`EdgionPlugins.spec.waf` 固定引用该策略，
通过 HTTPRoute 的 ExtensionRef 引用。

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
    policyRef:
      name: application-waf-policy
    activeProfileRef:
      name: application-waf-selector
```

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: application-waf-policy
  namespace: default
spec:
  data:
    type: WafPolicy
    config:
      profiles:
        baseline:
          bundleRefs:
            - name: application-rules-v1
              expectedDigest: a30e47998cace9b73f037d44f3fc9f0a29614e1753e07e6ea9f5c4e4c4d6b22d
      defaultProfile: baseline
```

## 配置

| 字段 | 必填 | 默认值 | 含义 |
|------|------|--------|------|
| `policyRef.name` | 是 | — | 固定引用 `WafPolicy` EdgionConfigData |
| `policyRef.namespace` | 否 | EdgionPlugins 的命名空间 | 策略所在命名空间；跨命名空间需满足可见性或 ReferenceGrant |
| `activeProfile` | 否 | `WafPolicy.defaultProfile` | 插件固定选择的 profile 名称；与 `activeProfileRef` 互斥 |
| `WafPolicy.profiles` | 是 | — | 1–16 个具名配置，每个配置包含 1–16 个有序 `bundleRefs`，至少一个为必需引用 |
| `WafPolicy.defaultProfile` | 是 | — | 没有可用 Selector 选择时使用的配置 |
| `activeProfileRef` | 否 | — | 插件引用有类型的 `Selector` EdgionConfigData，独立切换已配置的 profile；与 `activeProfile` 互斥 |
| `WafPolicy.profiles.<name>.bundleRefs[].name` | 每个引用必填 | — | 目标规则包 EdgionConfigData 名称 |
| `WafPolicy.profiles.<name>.bundleRefs[].namespace` | 否 | WafPolicy 的命名空间 | 目标命名空间；跨命名空间引用遵守 `visibility` 和 `ReferenceGrant` |
| `WafPolicy.profiles.<name>.bundleRefs[].expectedDigest` | 否 | — | 可选的规则包内容锁定值；省略时有效的规则包更新会自动生效 |
| `WafPolicy.profiles.<name>.bundleRefs[].optional` | 否 | `false` | 独立附加包缺失或无效时跳过并报告 `PartiallyInvalid`；基础包不可设为可选 |
| `mode` | 否 | `on` | `on` 执行规则阻断；`detectionOnly` 仅记录规则命中 |
| `priority` | 否 | `0` | 0–65535，仅在同一挂载范围内按较大值优先 |
| `requestBody.inspection` | 否 | `prefix` | `prefix` 提前检查指定窗口；`full` 检查完整有界请求体 |
| `requestBody.prefixSize` | 否 | `64KiB` | 前缀窗口，1 字节至 `1MiB`，支持 `16KiB`、`100k`、`1m` 等大小字符串；仅用于 `prefix` |
| `requestBody.json` | no | engine defaults | Complete JSON checker limit overrides; only valid with `full` |

规则源码不会写入错误响应、日志或指标，网关管理接口将其脱敏。

## 共享规则包

规则需要 `Include`、`@pmFromFile`，或多个 WAF 配置共用同一组规则时，使用有类型的
`EdgionConfigData`。一个规则包把有序入口文件、规则文件和词组资源放在同一版本的资源中。
下面的入口文件引用另一个规则文件；后者读取同包内的词组资源：

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: application-rules-v1
  namespace: default
spec:
  enable: true
  data:
    type: WafRuleBundle
    config:
      version: "2026.1"
      profile: baseline
      provenance: operations/revision-1
      digest: a30e47998cace9b73f037d44f3fc9f0a29614e1753e07e6ea9f5c4e4c4d6b22d
      roots: [main.conf]
      rules:
        - name: main.conf
          content: |
            Include child.conf
        - name: child.conf
          content: |
            SecRule REQUEST_URI "@pmFromFile phrases.data" "id:2001,phase:1,deny,status:403"
      phraseAssets:
        - name: phrases.data
          content: |
            /blocked
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: application-waf
  namespace: default
spec:
  waf:
    policyRef:
      name: application-waf-policy
```

| 规则包字段 | 必填 | 含义 |
|------------|------|------|
| `version`、`profile`、`provenance` | 是 | 非空的版本、规则集 profile 和来源说明；三者都参与摘要计算 |
| `digest` | 否 | 可选的规则包内容自检值；直接管理的规则可省略 |
| `roots` | 是 | 按顺序编译的入口规则成员名称，至少一个 |
| `rules[].name`、`rules[].content` | 是 | 相对成员路径及 SecLang 源码，至少一个规则成员 |
| `phraseAssets[].name`、`phraseAssets[].content` | 否 | 供 `@pmFromFile` 读取的相对成员路径及文本 |

摘要涵盖 `version`、`profile`、`provenance`、有序的 `roots`，以及分别按名称排序的规则成员和词组成员。
每个字符串前写入 8 字节大端长度，每个列表前写入 8 字节大端成员数；哈希输入以
`edgion-waf-rule-bundle-v1` 加零字节开头。成员名称必须是安全相对路径，不允许空、`.`、`..`
路径段、反斜杠、冒号或控制字符。规则和词组成员名称不能重复，所有入口必须指向规则成员。
规则包有固定的成员数及字节数上限，最终组合还必须通过严格编译。

所选 profile 按 `bundleRefs` 顺序、每个包的 `roots` 顺序编译。
`Include` 和词组资源只能读取发起规则所属包中的成员。
所选包中的规则 ID 重复会使整个版本编译失败。控制面（Controller）校验引用及已配置的摘要锁定值，
数据面（Gateway）在发布前重新校验收到的内容。引用缺失、被禁用、类型不符、未授权或内容无效时，
不能发布不完整的基础规则。独立附加包可标记 `optional: true`，缺失或无效时跳过，
并报告 `PartiallyInvalid=True`；最终组合仍须严格编译。必需包失效时保留上一有效版本。
`WafRuleBundle` 不采用 overlay 的 fail-to-base 策略。

需要多个版本或敏感等级时，分别建立具名 profile，例如 `low`、`mid`、`high`。
每个 profile 显式列出规则包和评分参数；这些名称不是内置 CRS 等级，须逐组合核验
规则 ID 和阈值。同一个 profile 可按顺序聚合多个包，但每个请求只运行被选中的一个 profile。

动态切换时，在 `EdgionPlugins.spec.waf` 中设置 `activeProfileRef: {name: application-waf-selector}`，
再创建 `data.type: Selector`、`data.config.active: high` 的 EdgionConfigData。
修改 `active` 可在控制面校验后切换；引用缺失、禁用、类型不符或选择未知 profile
时回到 `defaultProfile`，不会关闭 WAF。
固定选择时可在插件中改用 `activeProfile: high`，名称必须存在于策略中。
两个选择字段都不配置时使用 `defaultProfile`。

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: application-waf-selector
  namespace: default
spec:
  data:
    type: Selector
    config:
      active: high
```

对于已锁定摘要的引用，修改规则包内容不会自动启用新版本：先创建新的版本化规则包并计算摘要，再更新各 WafPolicy 引用的
`name` 和 `expectedDigest`。还有使用者固定旧版本时，应保留旧资源。未设置 `expectedDigest` 时，
有效的规则包更新会在 Controller 和 Gateway 校验后生效。启用跨命名空间引用校验时，
目标资源的 `visibility: Namespace` 需要在目标命名空间配置对应 `ReferenceGrant`；
`visibility: Cluster` 允许所有命名空间引用。可以写入被引用规则包的主体也能改变 WAF 执行策略，
应限制其写入权限。Admin 输出和 Rust `Debug` 会隐藏规则及词组正文，可信配置同步仍会将原文发送到 Gateway。

## 提交前检查规则支持情况

对单个 `EdgionPlugins` YAML 文件运行本地命令：

```sh
edgion-cli waf inspect --file application-waf.yaml --format json
```

此命令无需连接 Controller，也无需令牌。输入上限为 16 MiB，输出为带版本号的报告。
`analysisComplete` 只表示 SecLang 源码完成了兼容性分析，**不表示可以发布**；
只有 `strictValidationPassed: true` 才表示该 WAF 配置通过与 Controller、Gateway
相同的严格编译及请求体检查。未通过时命令以非零状态退出。
配置含 profile `bundleRefs` 时，本地命令没有 Controller 解析后的规则包内容，
因此会报告引用尚未解析，不会声称该配置可发布。提交资源后查看其状态以获得完整结果。

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

将这些指令放入 profile 引用的 WafRuleBundle 成员中，并在 WAF 插件中设置
`requestBody.inspection: full`：

```text
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

将这些指令放入 profile 引用的 WafRuleBundle 成员中，并在 WAF 插件中设置
`requestBody.inspection: full`：

```text
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

## Bounded JSON inspection

Full request-body inspection always uses the native JSON checker for effective
JSON processors. Omitting `requestBody.json` selects deterministic defaults;
this field supplies a complete override profile. Zero is restrictive, and every
field in an explicit profile is required. Legacy `document`, `plan` and `facts`
fields are rejected.

The Gateway proves complete bounded capture before one canonical body read.
Resident contiguous bodies share their existing Bytes allocation. Spilled or
edited bodies may require canonical BodyStorage materialization. The checker
scans that stable source once, matches precompiled rules as values complete,
and retains only required names, matching results and explicitly named deferred
values. It does not build a complete JSON value tree. Raw REQUEST_BODY visibility
remains controlled by the existing processor and ctl rules.

Lexical and structural retained-byte limits count dynamic heap capacity, excluding
fixed checker storage. Keys and paths each have 256 bytes of inline scratch, and
token decoding has 1024 bytes; these are shared across nesting levels. Spill
allocations are fallible and fully charged. A zero heap budget can still admit
inline data, while source, decoded-byte, key, path, depth and work limits remain
independently enforced. Rule selection scratch is reused and counts toward the
runtime aggregate retained-byte limit, including while evaluating a chain branch.

| Domain | Default limits |
|---|---|
| lexical | source/raw token 16 MiB; decoded/retained 32 MiB; events 131072; depth 64; work 128 MiB |
| structure | retained 8 MiB; keys 4 MiB; members 65536; path 16 KiB; depth 64; work 128 MiB |
| results | candidates 65536; facts 1000000; retained 32 MiB; work 128 MiB; deferred entries 4096; deferred bytes 4 MiB |

```yaml
requestBody:
  inspection: full
  json:
    lexical:
      maxSourceBytes: 16777216
      maxRawTokenBytes: 16777216
      maxDecodedBytes: 33554432
      maxEvents: 131072
      maxDepth: 64
      maxRetainedBytes: 33554432
      maxWork: 134217728
    structure:
      maxRetainedBytes: 8388608
      maxKeyBytes: 4194304
      maxMembers: 65536
      maxPathBytes: 16384
      maxDepth: 64
      maxWork: 134217728
    results:
      maxCandidates: 65536
      maxFacts: 1000000
      maxRetainedBytes: 33554432
      maxWork: 134217728
      maxDeferredEntries: 4096
      maxDeferredBytes: 4194304
```

`results.maxFacts` bounds allocated candidate/profile/change/outcome/capture
record slots, including reusable free slots and spare vector capacity. It does
not count only matching rules or live matches. Name/value storage, state bitmaps
and the named-demand index instead consume the retained-byte/work budgets.

Physical inspection limits are independent of logical WAF and Gateway body
limits. A physical failure rejects in DetectionOnly too and cannot be retried
through another parser. Malformed JSON retains the existing DetectionOnly
metadata behavior. Duplicate keys replace earlier subtrees; malformed syntax and
physical work already consumed cannot be erased by later duplicate replacement.
Full configurations with unsupported broad dynamic JSON value dependencies are
rejected during validation, including configurations using default JSON limits.
Use finite named dependencies where later transaction values are required.

Logical publication checks use fixed priority: source/prelude, name length,
depth, entries, decoded bytes, traversal work, then combined totals. Each limit
remains independent; competing failures may differ from the former DOM parser.
Depth is independently configurable; there is no historical 127-container guard.
WAF runtime-byte metrics represent logical transaction accounting, not physical
inspection capacity or RSS. Prefix inspected-byte metrics exclude full scans.


### 响应和配置更新

Phase 3 在响应头发送给客户端前执行。主上游响应始终流式处理，配置不允许完整
`RESPONSE_BODY` 聚合；phase 4 使用有界响应语义，phase 5 对正常完成的事务执行一次。
日志只记录有界规则 ID 和计数，不记录命中值或正文。

新配置先完整编译再原子发布；进行中的请求保留原规则版本。无效更新报告
`Accepted=False` 并保留最近可用版本。删除资源后，新请求不能继续解析该资源。
同一请求按 Route、Gateway、全局依次选择 WAF；仅在同一范围内比较 `priority`，
相同优先级按声明顺序决定。
后端级 WAF 配置不受支持，因为 WAF 请求阶段先于后端选择。

编译器严格拒绝未实现的指令、操作符、动作、无法解析的外部资源及完整响应体要求，
不能据此宣称兼容所有 ModSecurity 或 CRS 配置。
