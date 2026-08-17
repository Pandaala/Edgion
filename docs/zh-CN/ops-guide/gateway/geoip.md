# GeoIP 地理限制

> **Gateway 本地特性。** GeoIP 配置位于 Gateway 的本地启动配置中（不是 CRD），因为数据库、其内存映射
> 以及后台更新任务是进程级的单一资源，无法针对每个具名的 `EdgionGatewayConfig` 实例分别配置。
> 修改任意 `geoip` 字段都需要重启 Pod。

GeoIP 允许你按客户端的地理来源限制流量，并在访问日志中补充解析出的国家/大洲/城市信息。它扩展了两个
已有机制，而不是新增插件：

- **HTTP** — 在已有的 `RequestRestriction` 和 `GlobalAccessControl` 插件上新增两个
  `RestrictionSource` 取值（`ClientGeo`、`DirectGeo`）。
- **L4（TCP/TLS）** — 在已有的 `GlobalConnectionIpRestriction` Stream 插件上新增可选的 `geo` 块。

**设计上不支持的能力**：不内置基线数据库（受限于数据集的再分发条款）、不支持坐标/半径地理围栏
（MMDB 坐标是城市中心点值，`accuracy_radius` 精度较粗）、不支持 ASN 查询，也不会因数据库不可用而
阻塞就绪探针（见[无就绪门控](#无就绪门控运维方自行承担冷启动窗口)）。

## 配置

`geoip` 是 Gateway 本地 `GatewayConfig` 中的一个块 —— **必须嵌套在顶层的 `gateway:` 键下**，
不能写在文档根层级：

```yaml
# edgion-gateway.yaml
gateway:
  geoip:
    db_path: /var/lib/edgion/geoip/db.mmdb
    # 留空/不填 => 只加载本地文件，永不启动更新任务。
    update_url: https://mirror.internal/geo.mmdb
    # 环境变量名，其值会原样作为 Authorization 头发送。
    # 凭据永远不能放在 update_url 或 CLI 参数中。
    update_auth_header_env: GEOIP_UPDATE_AUTH
    update_interval_sec: 86400
    retry_interval_sec: 600
    connect_timeout_sec: 10
    download_deadline_sec: 600
    stall_timeout_sec: 60
    max_db_bytes: 268435456        # 256 MiB
```

### 🚨 最容易踩的坑

`GatewayConfig` 在根配置结构中通过 `#[command(flatten)]` 提供给 CLI，但在 YAML 侧**没有**
`#[serde(flatten)]`。这意味着把 `geoip:` 块写在**文档根层级**——比正确位置高了一层——依然能被
正常反序列化，但会被外层结构体悄悄丢弃，最终 `GeoIpConfig` 停留在空的默认值上（没有 `db_path`，
没有 `update_url`）。**没有任何错误、没有警告、启动日志也不会有任何提示。** 这不属于
`deny_unknown_fields` 能捕获的错误类型，因为 `deny_unknown_fields` 是加在 `GeoIpConfig` 自身上的
——未识别的键是在它上面一层、持有 `geoip` 字段的那个结构体上被丢弃的，`GeoIpConfig` 自己的
反序列化逻辑根本没有机会运行。

```yaml
# 错误 —— 静默产生一个空的 GeoIpConfig；启动时无任何报错
geoip:
  db_path: /var/lib/edgion/geoip/db.mmdb

# 正确 —— 嵌套在 gateway: 下
gateway:
  geoip:
    db_path: /var/lib/edgion/geoip/db.mmdb
```

这个坑在实现过程中真实地消耗了大量排查时间——它最初被误判为 conf_sync 的传播竞态问题，因为部署的
其他所有方面看起来都是正常的。如果地理规则看起来毫无效果，且 `edgion_geoip_db_loaded` 一直是
`0`，而你的 YAML 明明设置了 `db_path`，请先检查缩进层级。

### 字段参考

所有 `geoip` 键都是 **snake_case**（与 `GatewayConfig` 其余部分一致，不同于面向 CRD 的资源使用
camelCase）。该结构体带有 `#[serde(deny_unknown_fields)]`，因此 `geoip` 块**内部**的 camelCase
拼写错误（例如把 `db_path` 写成 `dbPath`）会导致硬性的配置加载错误，而不是静默使用默认值——这是
唯一能真正捕获拼写错误的层级。上面提到的陷阱发生在更高的一层，那里没有 `deny_unknown_fields`
来兜底。

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `db_path` | 路径 | 无 | 本地 MMDB 路径。首次启动时可能不存在，见[数据库文件生命周期](#数据库文件生命周期)。没有独立的总开关：`db_path` 未设置就意味着没有数据库，地理查询会报告 `Unavailable`。 |
| `update_url` | string | 无 | 更新源。留空/不填表示只加载本地文件，永不启动更新任务。在所有日志行中都会被脱敏。 |
| `update_auth_header_env` | string | 无 | 环境变量名，其值会原样作为 `Authorization` 头发送。在 Kubernetes 中通过 `secretKeyRef` 从 Secret 注入。 |
| `update_interval_sec` | u64 | `86400` | 首次成功加载**之后**，两次更新周期之间的稳态间隔。 |
| `retry_interval_sec` | u64 | `600` | 数据库**已经成功加载过一次之后**，失败重试的稳态退避时间。首次拉取不使用该值，见[冷启动退避](#多副本更新行为抖动和-meta-校验器)。 |
| `connect_timeout_sec` | u64 | `10` | 更新请求的 TCP 连接超时。 |
| `download_deadline_sec` | u64 | `600` | 单次下载尝试的整体截止时间。与客户端级别的总超时不同——后者刻意留空，见下文。 |
| `stall_timeout_sec` | u64 | `60` | 下载在此时长内无任何进展则中止。 |
| `max_db_bytes` | u64 | `268435456`（256 MiB） | 流式下载**过程中**强制生效的硬上限。 |

更新器的出站 HTTP 客户端通过项目共享的 `build_outbound_client` 辅助函数构建，因此走的是与其他所有
Gateway 发起的出站调用相同的出口策略（代理处理、重定向拒绝、DNS 解析器）。客户端自身的总请求超时
被刻意留空——否则在较慢的镜像链路上，60~80 MB 的数据库会导致每一个周期都超时中止——真正约束请求的
是 `download_deadline_sec` / `stall_timeout_sec`。

### 数据库文件生命周期

**镜像不携带任何数据库。** 首次启动时 `db_path` 不存在是正常的"尚未就绪"状态，而不是故障：
provider 保持为空，查询返回 `Unavailable`；如果配置了 `update_url`，后台更新器会拉取文件并热替换
进去。文件能否在 Pod 重启后保留，取决于你的部署方式——`emptyDir` 每次重启都会重新下载；PVC 或
`hostPath` 能在重启间保留文件。

**永远不要原地写 `db_path`。** `kubectl cp`、initContainer 里的 `cp`、projected volume 内容更新，
或任何截断重写该文件的脚本，都会导致 Gateway 进程发生 **SIGBUS**，因为它把该文件内存映射着。
SIGBUS **不是** Rust panic——它不会被 `catch_unwind` 捕获，不会执行析构函数，会直接杀死进程，
且是在处理请求的过程中，波及该 Gateway 实例上所有正在进行的连接。这是一条硬性的运维约束，
不是风格建议。

唯一被支持的数据库文件更新方式是：

1. 内置更新器（配置了 `update_url`），它会写入 `.tmp` 文件后 `rename(2)` 到位——这是一个原子操作，
   不会打扰正在读取现有 mmap 的进程。
2. 采用相同做法的外部写入者：在与 `db_path` **相同的文件系统**上写入临时文件，然后
   `rename(2)` 覆盖 `db_path`。跨文件系统的"临时文件+重命名"**不是**原子操作（会退化为复制+
   删除），会重新引入这条规则本应避免的确切风险——Gateway 在启动时会为自己的更新器显式检查这一点，
   如果交换目录不可用会拒绝启动更新任务，但它无法检查外部写入者的行为。

**更新卷的大小应按数据库的 3 倍规划，而不是 1 倍。** 在一次交换过程中，卷会同时持有：旧文件的
inode（已从目录中 unlink，但仍被 Gateway 的 mmap 引用，直到最后一个正在处理它的请求结束）、
新下载的文件，以及——如果上一个周期失败了——一个残留的 `.tmp`。更新器确实会在启动时和每个周期
开始时清理残留的 `.tmp`，也确实会在流式下载前根据 `max_db_bytes` 做剩余空间预检查，但如果卷本身
容量不足，这些都无济于事：一个放不下的 `.tmp` 会导致后续每一个周期都以同样的方式永久失败，
直到卷被扩容或 `.tmp` 被手动删除。

### 更新校验

下载得到的 `.tmp` 文件**只做结构性校验**：MMDB reader 必须能打开它，`binary_format_version`
必须合理，`node_count` 必须非零，`build_epoch` 必须是合理的时间戳，`database_type` 必须非空，
搜索树必须可遍历。**不存在任何形式的内容层面探测**，即便是运维方可配置的探测也没有——对于任意
运维方提供的 MMDB（区域裁剪版镜像、纯 IPv6 构建、仅覆盖 RFC1918 内网空间的测试库），没有任何
固定地址能保证一定能被解析出来，因此无论是硬编码的探测还是运维方自定义的探测，都会永久拒绝一个
只是恰好解析不出所配置地址、但本身完全合法的数据库。校验失败永远不会执行 rename；半损坏的数据库
严格来说比继续保留旧数据库更糟。

## HTTP 层规则

`RequestRestriction` 和 `GlobalAccessControl` 插件上的 `RestrictionSource` 新增了两个取值：
`ClientGeo` 和 `DirectGeo`。两个插件共享同一套谓词模型（`request_match.rs`），所以地理规则在
两个插件中行为一致。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-block
  namespace: production
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Blacklist
        ruleGroups:
          - name: block-ru
            rules:
              - source: ClientGeo
                key: country
                values: ["RU"]
        failOpen: false
```

- **`source`**：`ClientGeo` 读取解析后的真实客户端地址（经过 `X-Forwarded-For` / PROXY
  protocol 解析——与配置好的 `RealIp` 插件暴露的地址相同）。`DirectGeo` 读取直接的 TCP 对端地址，
  不受任何请求头影响。
- **`key`**：`country` | `continent` | `city` | `subdivision` 之一。每条地理规则都必须提供；
  缺失或拼写错误会在接入时被判定为 schema 错误并拒绝。
- **`values`**：对 `country`/`continent`，取值和查询结果都会被规范化为**大写**，并对照静态的
  ISO 3166-1 alpha-2 / 大洲代码表校验——未识别的代码只会产生 **WARN**，不会被拒绝（宽松接入策略：
  规则依然按原样生效）。`country`/`continent` 这两个 key 上会直接**拒绝** `caseSensitive`
  （不区分大小写的匹配路径会折叠为小写，与这里的大写规范化方向相反，允许该开关会悄悄产生一条永远
  无法匹配的规则）。`city` 和 `subdivision` 是自由文本，保留插件常规的 `caseSensitive` 语义。
- **地理来源上会拒绝 `regex` 和 `secretRef`**。查询结果始终是一个精确代码——允许 `regex` 能顺利
  编译通过，却永远不会匹配任何东西，因为没有什么值得用正则去匹配的自由格式字符串。
- **`failOpen`** 位于*插件配置*上（`RequestRestrictionConfig.failOpen` /
  `GlobalAccessControlConfig.failOpen`），而不是单条规则上。当插件内任意分组（对
  `GlobalAccessControl` 而言，任意 profile）中存在至少一条 `ClientGeo`/`DirectGeo` 规则时，它是
  **必填**、无默认值的；当插件不含任何地理规则时，它会被**拒绝**。它的含义、为何是插件级而非
  规则级、以及代价见下方 [`failOpen`](#failopen数据库不可用时的处置方式)。

### 没有 `registered_country` 兜底

当 `country.iso_code` 缺失时，刻意不回退到 `registered_country` 或 `represented_country`。
卫星 ISP、anycast 地址段、部分匿名化网络，以及某些军事/使馆分配段，都存在已填充的
`registered_country`（**注册方**）但无法解析出 `country`（**客户端的真实位置**）——如果混用
两者，会导致同一条 `country` 规则对不同客户端表达出不同的含义。这些客户端会解析为
`country: None`，进而走到下面的 `onMissing` 分支。如果你的流量中卫星或 anycast 客户端占比较高，
预期它们中的一部分会按设计永久无法解析，而不是遇到 bug。

`onMissing` 以同样的方式处理两种不同的"缺失"情况：该地址在 MMDB 中完全没有记录，以及记录存在但
缺少这一具体维度（对 `subdivision`/`city` 来说非常常见——很多携带 city/subdivision 数据的记录
只填充 `geoname_id` + 名称，没有 `iso_code`，`city.names` 对不常见的地方也经常缺少 `"en"` 键）。

### `city` 只匹配英文名称

`city` 取自 `city.names["en"]`——英文本地化名称，永远不是翻译后的名称。用
`values: ["北京"]`（或任何非英文拼写）写的城市规则永远不会匹配，因为查询结果始终是英文形式。
`city` 和 `subdivision` 也**不是全局唯一的**——"Cambridge" 在多个国家都存在，单独的
`subdivision: ["CA"]` 会同时匹配加利福尼亚州和任何代码为 `CA` 的加拿大省份。请始终在同一个 AND
分组内将 `city` 或 `subdivision` 与 `country` 配对使用：

```yaml
ruleGroups:
  - name: cn-beijing
    rules:
      - source: ClientGeo
        key: country
        values: ["CN"]
      - source: ClientGeo
        key: city
        values: ["Beijing"]
failOpen: false
```

### `failOpen`——数据库不可用时的处置方式

`onMissing` 无法表达"失败即拒绝"：在黑名单模式下，谓词为假意味着*放行*，所以"数据库尚未加载"和
"在它加载之前全部拦截"这两件事都无法用 `onMissing` 表达。`failOpen` 将这一情况单独拆分为一个
独立的、必填的、**插件级**短路开关——它位于 `RequestRestrictionConfig`/
`GlobalAccessControlConfig` 上，而不是单条规则上：

| 情况 | 由谁决定 |
|---|---|
| 完全没有加载数据库（`GeoLookup::Unavailable`） | 插件的 `failOpen` |
| 数据库已加载，但没有该地址的记录，或记录缺少该维度 | `onMissing` |

早期版本把 `geoFailOpen` 放在每条规则上，但运行时行为其实一直是插件级的：插件内任意一条规则
声明 `false`，就会让*整个插件*短路，与 `mode` 或它属于哪个分组无关——因此规则级的取值只能自己
和自己一致，同一插件内"规则 A 遇库不可用放行、规则 B 拒绝"没有任何连贯的含义。现在字段被移到了
它实际生效的位置：每个插件一个 `failOpen`，当该插件含有至少一条地理规则（在任意规则分组，或对
`GlobalAccessControl` 而言在任意 profile 中）时必填，否则被拒绝。

- **`failOpen: false`** —— **插件直接短路到拒绝**，使用其配置的 `status`/`message`，
  **无视 `mode`（黑名单或白名单），也无视其他任何规则分组。** 这正是为什么它是一个独立的插件级
  开关，而不是谓词的结果：谓词返回"假"在黑名单模式下意味着*放行*，这与一个不可用的安全控制项
  理应默认执行的行为正好相反。
- **`failOpen: true`** —— 每一条地理谓词都会被整体从 AND 分组中移除，效果等同于 `onMissing: Skip`。

无论哪种情况，都会记录一条 `EdgionStatus` 到访问日志——因此"地理策略静默地没有生效"至少在
逐请求粒度上是可见的，尽管它无法到达 CRD 状态条件——`RequestRestriction`/`GlobalAccessControl`
在 Gateway 进程内编译，`Accepted` 状态条件只能由 Controller 写入，而 Controller↔Gateway
的同步协议没有数据面到控制面的通道，无法把一个 Gateway 进程级的运行时事实带回去。

**每种处置方式在冷启动窗口期间具体的代价**（刚发布、`emptyDir` 上还没有数据库的时期，或任何更新器
一次都没有成功过的时间段）：

- 在含有安全相关规则（例如"拦截这个国家"）的插件上使用 `failOpen: false`，意味着在数据库加载
  完成前，所有命中该规则分组的请求都会被**拒绝**——你用可用性换取了"该规则从未被静默绕过"的保证。
- `failOpen: true` 意味着在数据库加载完成前，所有请求都会被**放行**，就像这条地理谓词根本不
  存在一样——你用执行力换取了这段窗口期的可用性。如果该分组的其余部分全部是带
  `onMissing: Skip` 的地理谓词，且处于**白名单模式**，请参见下方的"全 Skip 陷阱"——这种特定组合
  产生的效果与单独理解 `failOpen: true` 时的直觉恰恰相反。

没有第三种选项，也没有默认值；你必须在每个含有至少一条地理规则的插件上显式写明，也不能在不含
任何地理规则的插件上写它。

### `ClientGeo` 可被伪造；`DirectGeo` 不可以

`ClientGeo` 的解析地址**来自 `X-Forwarded-For`**，属于客户端可控输入。只有当部署真正终结在可信
代理之后——即配置了 `RealIp` 插件，且其 `trustedIps` 集合确实精确匹配你真实的代理/CDN 出口
范围，同时连接的 TCP 对端也在该集合内——它才是可信的。如果这些条件不满足，`remote_addr` 就只是
TCP 对端地址的原样拷贝，`ClientGeo` 会（无害地）退化为与 `DirectGeo` 相同的值。

危险的配置是把 `trustedIps` 配得过于宽泛——极端情况就是一条默认路由：

```yaml
# 反面示例 —— 不要这样做
- type: RealIp
  config:
    trustedIps:
      - name: everyone
        cidrs: ["0.0.0.0/0"]
```

有了这个配置，**任何**客户端都可以发送 `X-Forwarded-For: 1.1.1.1`，被当作来自
`1.1.1.1` 所在的地理位置，从而完全绕过 `ClientGeo` 黑名单规则。这是一个在实践中很可能发生的
误配置——某个 CDN 发布的出口范围不完整或经常变化，运维方"为了保险"扩大了 `trustedIps`，却在不知
不觉中把地理判定的完全控制权交给了每一个客户端。

> **关于自动检测的说明。** 与本文档中的其他一些坑不同，目前**没有**任何接入时的校验会将
> `ClientGeo` 规则与所在监听器的 `RealIp`/`trustedIps` 配置做交叉检查。`ClientGeo` 是否可信
> 完全需要运维方自行核对同一路由/网关上的 `RealIp` 插件配置来确认。如果你的部署并没有真正终结在
> 一个可信、范围精确的代理之后，任何关乎安全的规则都应优先使用 `DirectGeo`——它总是读取原始
> TCP 对端地址，不会受请求头影响。

### `GlobalAccessControl` 的 `ipSource` 命名反转

如果你在 `GlobalAccessControl` 中同时使用地理规则和它自身的 IP/CIDR 前置条件，需要注意
`GlobalAccessControl` 的 `ipSource` 枚举与 `ClientGeo`/`DirectGeo` 的命名是**反转**的：
`ipSource: ClientIp` 解析出的是真实客户端地址（XFF 解析后的地址，与 `ClientGeo` 读取的地址
*相同*），而 `ipSource: RemoteAddr` 解析出的是原始 TCP 对端地址（与 `DirectGeo` 读取的地址
*相同*）。将 `ipSource: ClientIp` 与 `DirectGeo` 规则配对（或反过来将 `ipSource: RemoteAddr`
与 `ClientGeo` 配对），会使同一个 AND 分组从两个不同的地址做决策，且不会有任何提示。当分组的
`ipSource` 与其地理规则的来源出现这种不一致时，接入时会发出一条 **WARN**
（`GlobalAccessControl ipSource/geo disagreement`）——所以与上面 `ClientGeo`/`trustedIps` 那个
坑不同，这种特定组合确实会被自动标记出来。

### 不存在"数据库分级"这个概念——缺失的维度只是普通的"无值"

该特性早期草案曾把 MaxMind 的产品线划分为"City 级"与"Country 级"，并在一条 `city`/
`subdivision` 规则针对看起来像"Country 级"的数据库编译时发出警告。这一检查已被移除：
`database_type` 只是一段自由文本（`GeoLite2-City`、`GeoIP2-Enterprise`，或某个供应商自己的
OEM 名称……），并没有可靠的办法把任意字符串归类为"有 city 数据"还是"没有"——一个确实携带
city/subdivision 粒度、但其 `database_type` 字符串不包含可识别关键字的数据库，会触发一条
**错误**的警告，告诉运维方一条实际生效的规则是失效的，这比完全不警告更糟。

更诚实的模型是：无论由哪个产品构建，解析出的记录都是统一的。当记录携带该数据时
`city`/`subdivision` 是 `Some(...)`，不携带时是 `None`——原因可能是以下三种之一（数据库
从不携带该维度、这一条具体记录缺少该维度、或该地址根本没有解析出记录），这三者是无法区分
的。它们都只是普通的"值缺失"，和一个缺失的请求头或 Cookie 一样，由 `onMissing` 治理：

- `onMissing: NoMatch`（默认）——一个 `country` + `city` 的 AND 分组，如果 `city` 永远
  解析不出值，整个分组会静默地永远不匹配。在黑名单模式下，该限制会对所有客户端**实际失效**，
  且没有任何明显的错误提示。
- `onMissing: Skip`——同一个分组会静默地收窄为仅按国家判定。
- `onMissing: Match`——同一个分组仅凭国家谓词就会匹配所有客户端，因为缺失的 `city` 值会被
  当作命中。

这里没有任何接入时或 Gateway 日志级别的信号，因为 Gateway 侧根本没有可校验的东西：一条
`city`/`subdivision` 规则是否最终能解析出值，完全取决于运维方加载了什么数据库，而这在每一次
更新周期都可能发生变化。请查看访问日志中的 `geo` 块，确认当前加载的数据库实际能解析出哪些字段，
并对照你的数据提供商文档，为每一条 `city`/`subdivision` 规则显式选择 `onMissing`，而不要依赖
默认值。

**全 Skip 白名单拒绝风暴。** 在 `mode: Whitelist` 下，一个**完全**由带 `onMissing: Skip` 的
地理谓词组成的规则分组，在数据库尚未加载完成时评估，会使该分组的 `considered == 0`——匹配器会
将其视为"没有分组匹配"，于是**所有请求都会被拒绝**，返回 403 且没有明显的诊断信息
（该分组技术上确实有规则，只是全部被跳过了）。注意 `failOpen: false` 会**先于**这种情况
触发短路，并以一个可记录的原因拒绝——这是更好、更易诊断的结果。恰恰是 `failOpen: true`
与一个全地理谓词的白名单分组中每条规则都是 `onMissing: Skip` 的组合，才会在冷启动窗口期间
产生这种静默的 403 风暴。

## L4（TCP/TLS）地理限制

`GlobalConnectionIpRestriction` 每个 profile 新增了一个可选的 `geo` 块——一个显式的、需要主动
选择加入的嵌套结构（而不是两个隐式为空的列表），这样"该 profile 是否使用地理限制"就没有歧义：

```yaml
profiles:
  main:
    defaultAction: Deny
    allow:
      - name: office
        cidrs: ["10.0.0.0/8"]
    geo:
      allowCountries: ["CN", "HK"]
      denyCountries: ["RU"]
      failOpen: false        # 必填 —— 无默认值
```

`allowCountries` / `denyCountries` 至少要有一个非空；空的 `geo` 块会被拒绝。`failOpen` 没有
默认值，原因与 HTTP 侧插件级的 `failOpen` 相同——运维方需要显式声明处置方式。**没有 `geo` 块的
profile 行为与今天完全一致**——这是一个严格的扩展，不会改变已有 profile 的行为。

L4 层**只支持国家匹配**——不支持大洲、城市或省级细分。在连接这一层没有请求属性可供组合，因此
细粒度的地理规则既难以运维又容易过度拦截；城市级别的判定应该放在 HTTP 层。只包含 `geo` 块、没有
任何 IP `allow`/`deny` 条目的 profile 是合法配置——一个纯地理准入 profile。

### 评估顺序

地理限制是在已有的 IP 拒绝/放行判定**之后**评估的第二阶段，而不是合并进 IP 匹配器本身：

| # | 条件 | 结果 |
|---|---|---|
| 1 | IP 命中拒绝 | **拒绝**（终止） |
| 2 | IP 命中放行 | **放行**（终止——完全豁免地理判定） |
| 3a | 未加载数据库（`Unavailable`） | `failOpen: true` → 不表态，进入第 4 步；`false` → **拒绝** |
| 3b | 命中 `denyCountries` | **拒绝** |
| 3c | 配置了 `allowCountries` | 命中 → **放行**（终止）；未命中 → **拒绝**（终止） |
| 3d | 只配置了 `denyCountries`，未命中 | 不表态，进入第 4 步 |
| 4 | `defaultAction`，但**仅当 profile 不处于白名单模式时** | 白名单模式下：**`DenyByWhitelistMiss`**；否则：`defaultAction` |

其中两行是关键，也很容易搞反：

- **IP 白名单命中（第 2 行）会让客户端完全豁免地理判定。** 当同时配置了 `allow` 列表和 `geo`
  块时，已知良好的 IP 来源永远不会被拿去做国家检查——这个豁免正是将地理限制合并进这个插件（而不是
  另建一个插件）所带来的实际表达能力。
- **地理判定"不表态"永远不会在白名单模式下让 `defaultAction` 复活（第 4 行）。** 由于
  `defaultAction` 默认为 `Allow`，如果地理"不表态"的结果在白名单 profile 中落到了这一步，那么
  在一个正常工作的 IP 白名单上加一条 `denyCountries: ["RU"]`——本意是想拦截*更多*流量——反而会
  静默地**放行白名单之外的所有地址**，因为白名单未命中的分支根本没有机会被走到。实现专门针对这一
  情况做了保护；一个白名单 profile 加上 `denyCountries`，依然会像加地理限制之前一样，拒绝
  不在白名单内的地址。
- **`allowCountries` 命中是终止性的（第 3c 行）**——它永远不会落到 `defaultAction`。在一个
  `defaultAction: Deny` 的 profile（这个插件的安全不变式要求的保守基线）中，如果让
  `allowCountries` 命中继续往下走，会导致 `allowCountries` 谁都放不进来。

### PROXY protocol 与第一阶段地理判定看到的地址

`GlobalConnectionIpRestriction` 在一次连接的生命周期中最多会在三个位置运行，它对"客户端 IP"的
理解——以及由此产生的地理判定——因阶段而异：

| 阶段 | 地址来源 | 是否经过 PROXY protocol 解析 |
|---|---|---|
| 第一阶段（`ConnectionFilter`，在 `accept()` 时） | 原始传输层对端 | **否** |
| TCPRoute | 解析后的客户端地址 | 是 |
| TLSRoute | 解析后的客户端地址 | 是 |

PROXY protocol 的解析严格发生在 `accept()` **之后**，所以第一阶段的连接过滤器无法看到
PROXY 声明的客户端地址——它看到的是负载均衡器自己的地址。在启用了 PROXY protocol 的监听器上，
第一阶段的 IP 和地理规则因此会匹配负载均衡器的 IP，而 TCPRoute/TLSRoute 阶段会匹配真实客户端的
地址。**这是该插件既有的、被接受的特性**——它的 IP allow/deny 规则今天就已经是这样的行为——
地理判定刻意继承了同样的语义，而不是用特殊逻辑来区分对待；保持一个 profile 内 IP 和地理两部分
行为一致，比针对某个机制单独开特例更有价值。这个插件没有 `ipSource` 选项可以绕开这个限制。

### L4 地理拒绝永远不会被记录日志，也不携带客户端地址

一次 L4 地理拒绝**在任何阶段都不会产生日志条目**，也**不会在任何地方记录客户端地址**。唯一的信号
是计数器 `edgion_geoip_denied_total{layer,reason}`（见[指标](#指标)）。这是刻意为之的设计，
不是第一阶段限制导致的将就：pingora 会在其 accept 循环内直接丢弃第一阶段拒绝的连接，所以那里确实
无法产生日志条目——但*同样*的不记录规则被统一应用到了 TCPRoute 和 TLSRoute 阶段，即使这两个阶段
技术上是可以记录日志的。在这三个阶段保持一致的行为，被认为比逐阶段的细节更有价值，而且在连接洪泛
攻击下，逐连接的拒绝日志本身也是一个放大向量。HTTP 层的地理拒绝不受此规则影响——它们保留正常的
`EdgionStatus` 条目和访问日志记录，因为那里本来就存在逐请求的日志。

### 无就绪门控：运维方自行承担冷启动窗口

一次刚发布、还没有数据库的部署（`emptyDir` 上很典型）**不会**扣住就绪探针。`failOpen`（每个 L4
`geo` 块，以及每个 HTTP `RequestRestriction`/`GlobalAccessControl` 插件）就是用来声明应该发生
什么的：`true` 接受一段地理策略
未生效的窗口期，`false` 接受一段流量被拒绝的窗口期。首次加载前的退避（见下文）会把这段窗口限制在
几秒钟以内，而不是完整的 `retry_interval_sec`；`edgion_geoip_db_loaded` 是用来告警"这个状态是否
还在持续"的指标。

## 访问日志

没有全局开关：地理能力完全由插件驱动，访问日志只会报告本次请求中确实被某个插件或 L4 谓词查询过
的槽位。当一条路由至少配置了一条 `ClientGeo` 和/或 `DirectGeo` 谓词时，命中的请求的访问日志条目
会新增一个 `geo` 对象，其中只包含被查询过的槽位：

```json
"geo": {
  "client": { "country": "CN", "continent": "AS", "city": "Beijing" },
  "direct": { "country": "US" }
}
```

每个槽位（`client` = `ClientGeo`，`direct` = `DirectGeo`）都是一个独立的、逐请求的 `OnceLock`。
访问日志只读取某个插件在本次请求中已经解析过的槽位——它本身从不触发查询。因此完全没有地理规则的
路由不会产生任何 MMDB 查询；只配置了 `ClientGeo` 规则的路由只会报告 `geo.client`，不会出现
`geo.direct`。如果希望每条日志都同时包含两个槽位，配置一条会读取每个槽位的谓词即可（用
`onMissing: Skip` 的规则就足够，既能触发读取又不改变放行结果）。

### 坐标

经纬度没有开关：只要解析出的记录确实携带坐标，`geo` 槽位就会输出 `coord`；不携带时则不输出——
与 `geo` 对象其余部分"只读取插件已经解析过的内容"是同一条规则，没有额外的门控。存在时，坐标会以
单个 `coord` 字符串形式输出，并经过与 `client-addr`/`remote-addr` 相同的脱敏处理。

**`coord` 没有按 key 单独解除脱敏的机制。** 脱敏完全由进程级的敏感日志模式控制——一个
全有或全无的开关。允许部分 ext 日志字段选择不脱敏的按 `LogExtSource` 白名单机制在这里不适用；
`client-addr` 和 `remote-addr` 今天也无法单独解除脱敏，`coord` 遵循同样的规则。

## 指标

| 指标 | 类型 | 标签 | 含义 |
|---|---|---|---|
| `edgion_geoip_db_loaded` | Gauge | 无 | 已加载 GeoIP 数据库时为 `1`，否则为 `0`。在配置了地理规则的情况下对 `== 0` 告警——这是"安全控制项未在运行"的首要信号。 |
| `edgion_geoip_db_build_epoch_seconds` | Gauge | 无 | 已加载数据库的构建时间戳。对陈旧程度告警。 |
| `edgion_geoip_update_failed_total` | Counter | `reason`（`fetch` \| `encoding` \| `size_cap` \| `stall` \| `validation` \| `rename` \| `disk` \| `deadline`） | 按分类统计的后台**更新周期**失败。`disk` 表示磁盘空间预检查拒绝启动（还没有发出任何请求）；`deadline` 表示整个周期超过了 `download_deadline_sec`（即使在持续取得进展），区别于 `stall`（在 `stall_timeout_sec` 内没有任何进展）——两者对应相反的修复方式。 |
| `edgion_geoip_lookup_panic_total` | Counter | 无 | 一次逐请求的 MMDB 查询发生 panic（已捕获）并退化为 `NotFound`。与上面的 `edgion_geoip_update_failed_total` 不同：这是查询路径上的逐请求事件，不是更新周期失败。 |
| `edgion_geoip_denied_total` | Counter | `layer`（`stage1` \| `tcp` \| `tls`）、`reason`（`deny_country` \| `allow_miss` \| `db_unavailable`） | 被地理规则拒绝的 L4 连接。永远不携带客户端地址；HTTP 拒绝刻意不计入此处——它们已经出现在访问日志中。 |

## 多副本更新行为、抖动与 `.meta` 校验器

更新器的请求周期：一次条件 `GET`（`If-None-Match` / `If-Modified-Since`）；`304` 会休眠到下一个
间隔，不触碰磁盘；`Content-Encoding` 不是 `identity` 的 `200` 响应会被直接拒绝（更新器不做解压——
请让 `update_url` 指向一个纯 MMDB 文件，而不是 `.mmdb.gz`）；`identity` 编码的 `200` 响应在
`max_db_bytes`/`stall_timeout_sec`/`download_deadline_sec` 的约束下流式写入
`<db_path>.tmp`；成功后 `rename` → 重新 mmap → 热替换 → 将校验器持久化到 `<db_path>.meta`。

**退避分两种模式。** 在**首次**成功加载之前，重试使用较短的指数退避（1 秒起，倍增，上限 30 秒）——
`retry_interval_sec` 默认的 600 秒是为稳态调优的，如果用于首次加载，会让一个冷启动的 Pod 在实际上
不受保护的状态下持续十分钟。首次成功之后，普通失败使用 `retry_interval_sec` 加抖动。

**抖动同样作用于首次拉取，而不只是重试。** 如果没有抖动，200 个 Pod 的滚动重启会向镜像同时发出
200 个无条件的 GET 请求。不过光有抖动并不能让这些请求变成条件请求——这是下面 `.meta` 文件的
职责。

**`<db_path>.meta` 持久化了用于首次让请求变为条件请求的 ETag/Last-Modified 校验器。** 在
`emptyDir` 卷上，`.meta` 会在每次 Pod 重启时和数据库本身一样丢失——但如果 `.meta` 缺失、而
数据库文件**仍然存在**（一种合理的部分持久化场景，例如某个 PVC 刚开始被使用，或是一次手动恢复），
更新器会用数据库文件的 mtime 来填充 `If-Modified-Since`，而不是直接跳到无条件拉取。需要规划的
实际后果是：丢失 `.meta`——无论是因为 `emptyDir` 重启还是其他任何原因——都会让**下一次**更新
周期变成一次完整的无条件下载，而不是一次廉价的 `304`，即便远端内容其实并没有变化。

## 故障排查

| 现象 | 可能原因 | 解决方法 |
|---|---|---|
| `edgion_geoip_db_loaded` 一直是 `0`，但你的 YAML 明明设置了 `geoip.db_path` | `geoip:` 写在了文档根层级，而不是嵌套在 `gateway:` 下 | 把它移到 `gateway:` 下——见[最容易踩的坑](#-最容易踩的坑) |
| 配置加载失败，报错提到某个 `geoip` 子字段是未知字段 | `geoip` 块内部的 camelCase 拼写错误（例如 `dbPath`） | 使用 snake_case——`db_path`、`update_url` 等 |
| 全新部署之后 `edgion_geoip_db_loaded` 一直是 `0` | 首次启动时的正常状态——没有内置数据库，且更新器尚未完成第一个周期 | 等待首次成功周期，或检查 `edgion_geoip_update_failed_total{reason}` 是否卡在某个失败原因上 |
| Gateway 进程在没有 panic 日志的情况下崩溃，且发生在某个数据库文件刚被外部修改之后不久 | 有东西原地写入了 `db_path`，而不是用重命名做替换——SIGBUS | 停止任何原地写入的行为；只有内置更新器，或同文件系统上"临时文件+重命名"的写入方式被支持 |
| 更新周期持续以类似 `ENOSPC` 的错误失败 | 更新卷容量不足以完成一次交换（需要旧 inode + 新文件 + 可能的 `.tmp`） | 将卷扩容到数据库大小的 3 倍 |
| 黑名单模式下，`country` + `city` 规则分组永远不匹配任何内容 | 已加载数据库的记录不携带 city 数据，撞上了该规则的 `onMissing: NoMatch`——见[不存在"数据库分级"这个概念](#不存在数据库分级这个概念缺失的维度只是普通的无值) | 查看访问日志的 `geo` 块并对照供应商文档；显式设置 `onMissing`，或去掉 `city`/`subdivision` 谓词 |
| 冷启动期间，一个只包含地理规则的白名单模式 profile 上，所有请求都被 403 拒绝 | 全 Skip 白名单风险：分组中每条规则都是 `onMissing: Skip`，加上插件级 `failOpen: true`，且评估发生在数据库尚未加载完成之前 | 改用 `failOpen: false` 以获得可诊断的拒绝，或确保分组中至少存在一条非地理谓词 |
| 带翻译语言取值的 `city` 规则永远不匹配 | `city` 只会与 `names["en"]` 比较 | 使用英文城市名 |
| 黑名单模式的 `ClientGeo` 规则被任意客户端轻易绕过 | `RealIp` 的 `trustedIps` 配得过宽（例如 `0.0.0.0/0`）——`ClientGeo` 退化为攻击者可控输入 | 将 `trustedIps` 收窄到你真实的代理出口范围，或改用 `DirectGeo` |
| `GlobalAccessControl` 的一个分组在 IP 前置条件和地理规则之间判定不一致 | `ipSource: ClientIp` 与 `DirectGeo` 配对（或反过来）——GAC 的 `ipSource` 命名相对于 `RequestInfo` 是反转的 | 将 `ipSource: ClientIp` 与 `ClientGeo` 配对，`ipSource: RemoteAddr` 与 `DirectGeo` 配对——Gateway 也会对这一具体的不匹配发出接入 WARN |
| 发生了一次 L4 地理拒绝，但任何地方都找不到对应的日志行 | 符合预期——L4 地理拒绝按设计永远不会在任何阶段被记录日志 | 使用 `edgion_geoip_denied_total{layer,reason}` 作为唯一信号 |
| 一个带 IP 白名单和 `denyCountries` 的白名单模式 L4 profile，似乎放行了不该放行的流量 | 检查被放行的客户端是否命中了 IP `allow` 列表——IP 放行命中是终止性的，会完全豁免地理判定（评估顺序的第 2 行） | 这是设计行为；如果豁免范围过宽，收窄 `allow` 的 CIDR |

## 相关文档

- [Gateway 配置](../configuration/gateway-config.md) —— 完整的 `GatewayConfig`
  schema 参考，包括 `geoip`
- [Real IP](../../user-guide/edgion-plugins/real-ip.md) —— 决定 `ClientGeo` 是否可信的
  `trustedIps` 配置
- [指标](../observability/metrics-inventory.md) —— 完整的指标清单，包括四个
  `edgion_geoip_*` 系列
