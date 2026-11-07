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
| `delegationPrecedence` | 否 | `child` | 仅用于路由委派链。`child`：更深层（子路由）的 WAF 替换本层；`parent`：本层 WAF 胜过它下面的所有路由。只对路由引用的 WAF 生效，写在 Gateway 级和全局 WAF 上会被忽略 |
| `requestBody.inspection` | 否 | `full` | `prefix` 提前检查指定窗口；`full` 检查完整有界请求体 |
| `requestBody.prefixSize` | 否 | `64KiB` | 前缀窗口，1 字节至 `1MiB`，支持 `16KiB`、`100k`、`1m` 等大小字符串；仅用于 `prefix` |
| `requestBody.maxInspectSize` | 否 | 网关物理上限 | 正数大小字符串；仅用于 `full`，限制检查窗口 |
| `requestBody.onLimit` | 否 | `continue` | 达到检查／存储策略上限时停止后续 WAF 阶段；`reject` 则返回 413 |
| `requestBody.json` | no | engine defaults | Complete JSON checker limit overrides; only valid with `full` |

规则源码不会写入错误响应、日志或指标，网关管理接口将其脱敏。

默认使用 `full` 检查。JSON/XML 的源字节上限统一取有效检查窗口，未配置时继承网关物理上限（默认 32 MiB）；不再提供 `json.lexical.maxSourceBytes`。独立的 token、堆内存和工作量预算仍然生效。普通 HTTP 即使没有 WAF 或关闭正文缓存，也执行物理上限；gRPC、CONNECT 和成功升级后的隧道流量除外，仅发送 Upgrade 请求头不能豁免。

## 静态规则选择

在每个 profile 的 `ruleSelection` 中写 YAML 条件，按可信配置选择规则。
Controller 校验后，Gateway 在发布新规则代际时重新编译；请求阶段不再计算这些条件。
下面的示例仅排除同时满足指定 tag、阶段和 ID 范围的规则，保留 `always-required` 规则：

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
      defaultProfile: application
      profiles:
        application:
          bundleRefs:
            - name: application-rules-v1
          ruleSelection:
            exclude:
              allOf:
                - tag: application-specific
                - phase: 2
                - ruleIdRange: {start: 100000, end: 100999}
                - not: {tag: always-required}
```

`include` 省略时为 `true`，`exclude` 省略时为 `false`。最终选择条件为
`include && !exclude`；排除优先，源码已移除的规则不会被重新启用。
省略 `ruleSelection` 或写 `{}` 会保留原来的编译路径；`null` 无效。

条件支持布尔值，或恰好包含一个键的对象：`tag`、正整数 `ruleId`、闭区间
`ruleIdRange: {start, end}`、`phase: 1..5`、非空 `allOf` / `anyOf` 列表，以及单个 `not` 条件。
Tag 按字面值精确匹配，区分大小写；不支持通配符、正则、宏或自动去空格。
规则链整体选择，ID 和阶段取链根，tag 来自合法成员的元数据；结构性 marker 不参与选择。
当前严格规则语言不允许给链子规则新增 `tag`。

每个 profile 的 include/exclude 合计最多 256 个节点、64 个元数据条件、16 层，
每个 tag 最多 256 个 UTF-8 字节；紧凑 JSON 最多 16 KiB，整份策略最多 64 KiB。
未知键、错误类型、空逻辑组和直接解码可见的重复键均被拒绝。
资源入口先经过通用 JSON 对象转换，原始重复键可能已被折叠；不要依赖重复键的结果。

排除不能掩盖无效规则：原始规则集仍须通过语法、语义、正则、词组资源和请求体策略校验。
无匹配 tag 和有效的空选择允许发布，Accepted 消息给出固定的数量摘要；
它们不会单独触发 `PartiallyInvalid`。修改选择条件会热更新规则，进行中的请求保留旧代际。

语言 profile 必须以整个后端栈的可信清单为依据。不要简单排除所有 `language-php` 或
`language-java`：部分规则还保护通用路径、SSRF 或跨语言攻击。
仓库 `config/waf/crs-4.25.1-supported-v1/language-selection-examples/` 提供按指定 CRS
版本审计的 Java/PHP 选择示例及逐项省略记录；使用前确认其后端假设，规则升级后重新审计。

## 检查本地 tag

`waf tags` 接受 1–16 个本地 WafRuleBundle EdgionConfigData YAML 文件。
`--selection` 文件只包含独立的 include/exclude 对象：

```bash
edgion-cli waf tags --bundle bundle.yaml --format json
edgion-cli waf tags --bundle setup.yaml --bundle core.yaml \
  --selection selection.yaml --selected --offset 0 --format json
edgion-cli waf tags --bundle core.yaml --tag language-php --format json
```

```yaml
include: true
exclude:
  allOf:
    - tag: application-specific
    - not: {ruleId: 100001}
```

报告区分语义目录、计划选择和成功编译后的成员。无效规则可能仍有完整 tag 目录，
但 `baselineCompilationPassed` / `selectedCompilationPassed` 会准确报告失败或未知。
本地检查不解析集群引用，也不校验具体插件的请求体模式；`bodyAdmissionChecked` 始终为 `false`。
Tag 查询的成员依据为 `semanticInventory`；选择查询在编译成功后为 `compiledSelection`。

所有输入合计最多 64 MiB，选择文件最多 64 KiB。报告最多 128 个 tag 行和
128 个根 ID 引用、64 KiB 输出；省略计数独立于实际选择结果。
单个 tag、`--selected` 或 `--excluded` 查询可用 `--offset` 按根 ID 升序分页。
规则源码、正则、词组内容和内部 Include 路径不会进入报告。

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

算子工作量（原始、变换后及检测器字节）按规则计算预算。chain 的所有成员，以及前序成员命中多个值时
产生的每个分支，共享同一份预算，因此匹配大量客户端可控值的 chain 不能成倍放大后续成员
的工作量。

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

Phase 1 在读取正文前检查请求头等信息。显式配置的 `prefix` 模式在选择或连接后端前，
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

`full` 只在完整文档检查完成后执行正文规则。可通过 `maxInspectSize` 设置更小的检查窗口；
仅 WAF 消费正文且尚无完整缓存时，新存储容量也会限制窗口。第 N+1 字节用于证明截断，
恰好 N 字节则必须确认真实 EOF。截断时丢弃未完成的 JSON/XML 结果，并停止所有剩余 WAF 阶段。
默认 `onLimit: continue` 原样转发正文；`reject` 返回 413，在 `detectionOnly` 下也生效。
Phase 1 结果保留，访问日志以 `inspection_stop.admitted_bytes` 和 `inspection`／`storage` 原因记录
窗口截断，不能把它当成完整检查通过。其他完整正文消费者仍要求完整捕获，其存储失败不会被此策略忽略。
已有完整缓存保留原容量。真实解析、传输和存储失败仍按错误处理；显式窗口大于请求固定的网关物理上限时拒绝执行。
全量模式不配置 `prefixSize`。网关物理正文上限和 WAF 执行预算始终独立生效。前缀放行后才发现超限时，后端可能已收到数据。
`detectionOnly` 不会忽略传输、存储或物理上限错误。

## 响应及热更新

## Bounded JSON inspection

Full request-body inspection always uses the native JSON checker for effective
JSON processors. Omitting `requestBody.json` selects deterministic defaults;
this field supplies a complete override profile. Zero is restrictive, and every
field in an explicit profile is required. Legacy `document`, `plan` and `facts`
fields are rejected.

The Gateway feeds actual capture chunks to the checker and proves bounded
completion before Phase 2. Cached bodies use bounded immutable snapshot chunks;
JSON-only spilled bodies need no complete materialization. The checker
uses one grammar pass, matches precompiled rules as values complete,
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
| lexical | source: effective inspection ceiling; raw token 16 MiB; decoded/retained 32 MiB; events 131072; depth 64; work 128 MiB |
| structure | retained 8 MiB; keys 4 MiB; members 65536; path 16 KiB; depth 64; work 128 MiB |
| results | candidates 65536; facts 1000000; retained 32 MiB; work 128 MiB; deferred entries 4096; deferred bytes 4 MiB |

```yaml
requestBody:
  inspection: full
  json:
    lexical:
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
metadata behavior. Every occurrence of a duplicate key is published as its own
`ARGS`/`ARGS_NAMES` value in document order and counts toward `&ARGS` and the
entry limits; a named macro such as `%{ARGS.json.key}` reads the first occurrence.
Full configurations with unsupported broad dynamic JSON value dependencies are
rejected during validation, including configurations using default JSON limits.
Use finite named dependencies where later transaction values are required.

Logical publication checks use fixed priority: source/prelude, name length,
depth, entries, decoded bytes, traversal work, then combined totals. Each limit
remains independent; competing failures may differ from the former DOM parser.
Depth is independently configurable; there is no historical 127-container guard.
The WAF runtime-byte gauge combines logical transaction accounting with known
provisional parser/result heap capacities during inspection. It excludes source
storage and allocator overhead and does not measure RSS.


### 有界 XML 检查

`requestBody.inspection: full` 下，实际选择 XML processor 时使用原生检查器。
自动选择识别 `application/xml`、`text/xml` 和以 `+xml` 结尾的媒体类型，忽略大小写及参数；
phase 1 的 `ctl:requestBodyProcessor=XML` 可以覆盖选择。HTTP Content-Type 参数（包括
`charset`）不驱动转码或 XML 编码校验；输入字节必须是 UTF-8，XML 声明的 encoding
若存在也只能为 UTF-8。下面配置引用已有 policy：

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
      inspection: full
    policyRef:
      name: application-waf-policy
```

在引用的 rule bundle 中，`XML` 选择元素文本及属性值，`XML:/*` 选择元素文本，
`XML://@*` 选择属性。例如：

```apache
SecRule XML:/*|XML://@* "@contains forbidden" "id:100001,phase:2,deny,status:403"
```

这些选择器按引擎的值投影工作，不支持任意 XPath。每个含字符内容的元素在关闭时产生一个值：
其完整文本内容，按文档顺序包含后代元素文本、CDATA 段和解码后的引用（即 ModSecurity
`XML:/*` 检查的 libxml2 节点内容）。注释和处理指令不贡献内容，也不会切分值，因此
`SEL<!---->ECT` 按 `SELECT` 检查。内容外围的字面 XML 空白被去除，CDATA 和字符引用产生的
空白保留；内容为空或仅含字面空白的元素不产生值。嵌套在多层元素中的文本会被每个外层元素
各检查一次。属性按原文的限定名称匹配，
包括 namespace 声明。命中名称是 `XML` 或 `XML:<attribute>`。注释、PI 和元素名不是值。
XML 不增加 ARGS 条目，combined-size 元数据只计算 query 参数。

支持范围是 UTF-8/XML 1.0，可带 BOM 和声明。检查结构、字符、namespace 绑定及重复属性，
规范化换行和属性的字面空白，支持五种预定义实体及合法数值引用。拒绝 DTD、外部或一般实体、
非 UTF-8 字节或 encoding 声明及未支持的 XML 路径；这不是 schema 校验。动态 XML value operand 在 Full 配置
校验时拒绝，动态 count operand 仍可用。已有 CRS 属性排除规则按原顺序生效。

| 范围 | 固定默认限制 |
|---|---|
| parser | source: effective inspection ceiling; raw token 16 MiB；name 16 KiB；decoded bytes 32 MiB；events 262144；depth 64 |
| structure | 每个 tag 的属性 65536；活动 namespace binding 65536；活动 name/namespace 字节 8 MiB |
| storage/work | parser heap 32 MiB、work 128 MiB；selected-result 限制与 JSON result 默认值相同 |

没有 `requestBody.xml` 配置项。物理检查限制与逻辑 WAF、Gateway body 限制分别生效。
连续的普通 value 优先引用原文，跨 chunk 或需要规范化的 value 使用可复用的内联缓冲区，
按上限扩容；不构建完整 XML 树或字段值映射。匹配、count 所需元数据及 capture 仍会占用
有界存储。

Full XML 与 JSON 共用 canonical BodyStorage 捕获及转发路径，XML-only spill 不需要
完整 materialization。raw 可见性和格式错误的 DetectionOnly fallback 保留原策略；
格式错误丢弃暂存 XML 命中，物理限制失败在 DetectionOnly 下也拒绝。
空 transport body 可以接受，非空的纯空白或注释文档仍需要 root。
由 XML 值产生的 phase 2 capture、chain、TX 和 control 效果在完整准入后按规则顺序执行。
phase 1 的效果在 body 检查前执行。

### 响应和配置更新

Phase 3 在响应头发送给客户端前执行。主上游响应始终流式处理，配置不允许完整
`RESPONSE_BODY` 聚合；当前 phase 4 在响应头提交前按逻辑结束执行，
不读取上游响应体；phase 5 对正常完成的事务执行一次。
日志只记录有界规则 ID 和计数，不记录命中值或正文。

新配置先完整编译再原子发布；进行中的请求保留原规则版本。无效更新报告
`Accepted=False` 并保留最近可用版本。删除资源后，新请求不能继续解析该资源。
同一请求按 Route、Gateway、全局依次选择 WAF；仅在同一范围内比较 `priority`，
相同优先级按声明顺序决定。在[路由委派](../../delegation.md)链上，每一层先按
`priority` 选出本层的 WAF；默认由有 WAF 的最深层胜出，除非某个祖先层的 WAF 设置了
`delegationPrecedence: parent`，此时设置了该项的最外层胜出。
后端级 WAF 配置不受支持，因为 WAF 请求阶段先于后端选择。

编译器严格拒绝未实现的指令、操作符、动作、无法解析的外部资源及完整响应体要求，
不能据此宣称兼容所有 ModSecurity 或 CRS 配置。

## 仓库提供的 CRS 4.25.1 支持集

导入 `config/waf/crs-4.25.1-supported-v1/generated/` 中的完整 YAML 资源到
`edgion-default`，再通过 HTTPRoute 的 ExtensionRef 关联
`EdgionPlugins/crs-supported`。示例默认选择 `standard`（PL1），`strict`
选择 PL4；两档都显式启用 UTF-8 校验和 XML 属性检查。建议先使用
`mode: detectionOnly` 校验业务正常流量，再启用 `on`。策略对必需的调优包
和核心包固定 digest；改动规则内容时需要重新生成并明确更新 pin。

该包保留 627 条原始核心规则中的 565 条，加上原始初始化 900990 和一条
单独的 Edgion 调优规则，共 567 条。明确排除 59 条响应体规则以及
901320/905100/905110：持久集合初始化、Apache 探测请求的审计关闭及豁免控制
仍不受支持。带相应 User-Agent 的 loopback 请求继续正常检查。
保留规则的原文字节及全部 21 个 phrase asset 都有固定来源和哈希，
这是一套明确的支持集，不代表完整 CRS 兼容。

原始初始化采用阈值 5/4 和 `early_blocking=0`，没有隐式增加参数长度限制。
strict PL4 可在响应头提交前通过响应状态规则拦截后端 500；standard PL1
不会启用该 PL2 检测。上游响应体不参与扫描。运行预算保持默认，
大 body 仍可能触发工作量或内存上限；DetectionOnly 不会忽略致命运行错误。

使用 `cargo run -p edgion-resources --example generate_supported_crs -- --check`
从仓库内的来源复核生成结果。规则包 README 和 `generated/acceptance.json`
记录排除项、来源 pin、profile 设置及输出 digest。


### JSON/XML 正文硬上限

可选配置 `requestBody.maxJsonBodySize` 和 `requestBody.maxXmlBodySize` 接受正数字节大小字符串，例如 `2MiB`。不配置表示没有额外格式限制，仍受网关正文上限约束。它们根据 Phase 1 后的实际正文处理器生效，不进行额外内容探测或解压；仅选中的 WAF 配置生效。

这两个上限适用于 `prefix`、`full` 和 `detectionOnly`，即使规则不读取正文，也会持续执行大小限制。它们同时检查已接收的原始正文和已有逻辑缓存，启用后禁止后续正文改写，并与修改正文的流式处理器互斥。前缀检查结束后仍继续计数，重放不重复计数。

超过上限时，尚未选定最终响应则返回 413；已选定响应时，沿用现有响应管线终止剩余上传，不替换已选定的响应。已发送到后端的字节无法撤回；需要保证后端零暴露时，应先完整缓存。格式硬上限与检查窗口不同，不能通过停止检查来绕过。
