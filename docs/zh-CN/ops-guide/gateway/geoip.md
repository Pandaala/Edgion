# GeoIP 地理限制

> **Gateway 本地特性。** GeoIP 数据库配置位于 Gateway 的本地启动配置中（不是 CRD），因为数据库、其内存
> 映射以及后台更新任务是进程级的单一资源，无法针对每个具名的 `EdgionGatewayConfig` 实例分别配置。
> 修改任意 `geoip` 字段都需要重启 Pod。判定能力本身是一个普通的 CRD 管理插件 `GeoIpLocation`，
> 和其他插件一样支持热重载。

GeoIP 允许你按客户端的地理来源限制流量，并在访问日志中补充解析出的国家/大洲/城市信息。两项能力都由
**一个插件 `GeoIpLocation`** 提供，以完全相同的配置结构注册进两层：

- **HTTP** — `EdgionPlugins.spec.requestPlugins` 中的 `EdgionPlugin::GeoIpLocation`。
- **L4（第一阶段 / `ConnectionFilter`，覆盖 TCP+TLS）** — `EdgionStreamPlugins.spec.plugins` 中的
  `EdgionStreamPlugin::GeoIpLocation`。没有第二阶段（`tlsRoutePlugins`）注册。

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
| `update_auth_header_env` | string | 无 | 环境变量名，其值会先按 HTTP `Authorization` 头校验；非法值会被拒绝，错误不会回显原值。在 Kubernetes 中通过 `secretKeyRef` 从 Secret 注入。 |
| `update_interval_sec` | u64 | `86400` | 首次成功加载**之后**，两次更新周期之间的稳态间隔。设置 `update_url` 时必须大于零。 |
| `retry_interval_sec` | u64 | `600` | 数据库**已经成功加载过一次之后**，失败重试的稳态退避时间。首次拉取不使用该值，见[冷启动退避](#多副本更新行为抖动和-meta-校验器)。设置 `update_url` 时必须大于零。 |
| `connect_timeout_sec` | u64 | `10` | 更新请求的 TCP 连接超时。设置 `update_url` 时必须大于零。 |
| `download_deadline_sec` | u64 | `600` | 单次下载尝试的整体截止时间。与客户端级别的总超时不同——后者刻意留空，见下文。设置 `update_url` 时必须大于零。 |
| `stall_timeout_sec` | u64 | `60` | 下载在此时长内无任何进展则中止。设置 `update_url` 时必须大于零。 |
| `max_db_bytes` | u64 | `268435456`（256 MiB） | 流式下载**过程中**强制生效的硬上限。设置 `update_url` 时必须大于零。 |

更新器的出站 HTTP 客户端通过项目共享的 `build_outbound_client` 辅助函数构建，因此走的是与其他所有
Gateway 发起的出站调用相同的出口策略（代理处理、重定向拒绝、DNS 解析器）。客户端自身的总请求超时
被刻意留空——否则在较慢的镜像链路上，60~80 MB 的数据库会导致每一个周期都超时中止——真正约束请求的
是 `download_deadline_sec` / `stall_timeout_sec`。

Authorization 值只解析一次，作为有类型的 HTTP header 存储，并在存储前标记为 sensitive。clone
会保留该元数据；它用于协作式的结构化 `Debug` 遮蔽，并让 HTTP/2 选择 never-indexed 编码。该元数据
不是加密或内存清零机制，也不承诺对序列化后的 HTTP/1 wire 日志做全面脱敏。

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

**更新卷的大小应按 `max_db_bytes` + 当前数据库大小的 2 倍规划，而不是 `max_db_bytes` 的 3 倍。**
在一次交换过程中，卷会同时持有：旧文件的 inode（已从目录中 unlink，但仍被 Gateway 的 mmap 引用，
直到最后一个正在处理它的请求结束）、新下载的文件，以及——如果上一个周期失败了——一个残留的
`.tmp`。剩余空间预检查与此完全对应：需要 `max_db_bytes`（正在流式写入的 `.tmp`，受该上限约束）
加上数据库**当前磁盘大小**的 2 倍（旧文件被计两次，覆盖 rename 前后的瞬时双持有），在还没有数据库
时退化为只需要 `max_db_bytes`。按 `max_db_bytes` 的 3 倍规划会在实际数据库小于该上限时（这是
常见情况）造成超额配置；而按上述公式仍然不足的卷，会在任何请求发出之前就以 `reason="disk"`
让每个周期失败。更新器确实会在启动时和每个周期开始时清理残留的 `.tmp`，但如果卷本身容量不足，
这些都无济于事：一个放不下的 `.tmp` 会导致后续每一个周期都以同样的方式永久失败，直到卷被扩容或
`.tmp` 被手动删除。

### 更新校验

下载得到的 `.tmp` 文件**只做结构性校验**：MMDB reader 必须能打开它，`binary_format_version`
必须合理，`node_count` 必须非零，`build_epoch` 必须是合理的时间戳，`database_type` 必须非空，
搜索树必须可遍历。**不存在任何形式的内容层面探测**，即便是运维方可配置的探测也没有——对于任意
运维方提供的 MMDB（区域裁剪版镜像、纯 IPv6 构建、仅覆盖 RFC1918 内网空间的测试库），没有任何
固定地址能保证一定能被解析出来，因此无论是硬编码的探测还是运维方自定义的探测，都会永久拒绝一个
只是恰好解析不出所配置地址、但本身完全合法的数据库。校验失败永远不会执行 rename；半损坏的数据库
严格来说比继续保留旧数据库更糟。

## `GeoIpLocation` 插件

`GeoIpLocation` 两处注册共用一套配置结构，有两种模式，完全取决于是否配置了任何规则分组：

```yaml
# 只解析 —— 把地理信息写入访问日志（仅 HTTP）；不做任何判定。
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-log
  namespace: production
spec:
  requestPlugins:
    - type: GeoIpLocation
      config: {}
```

```yaml
# 规则模式 —— 按地理维度拒绝/放行。
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-block
  namespace: production
spec:
  requestPlugins:
    - type: GeoIpLocation
      config:
        defaultAction: allow          # allow | deny
        failOpen: false               # 一旦配置了任何规则分组即为必填
        denyRuleGroups:
          - name: block-moscow
            rules:
              - source: DirectPeerIp
                key: city
                values: ["Moscow"]
        allowRuleGroups: []
```

完全相同的 `config` 块也可以作为 `EdgionStreamPlugins.spec.plugins`（第一阶段 /
`ConnectionFilter`）条目注册，用于 L4（TCP/TLS）判定，只有一处限制：`source: RemoteIp` 在 L4
会被拒绝（见下文）。

**把地理判定和其他规则组合在一起，是"链式编排"而不是"内嵌"。** `GeoIpLocation` 不携带 IP
白名单或请求头/正文谓词，`IpRestriction`/`RequestRestriction`/`RequestAccessPolicy`/
`GlobalConnectionIpRestriction` 也不携带地理谓词。"拦截这个国家，除非源 IP 在办公室白名单里"
这种需求，是通过把 `GeoIpLocation` 和 IP 插件作为同一个插件列表上**独立的、有顺序的条目**来表达
的：

```yaml
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
    - type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: block-ru
            rules:
              - source: DirectPeerIp
                key: country
                values: ["RU"]
```

### 规则形状

- **`source`**：`DirectPeerIp` 是直接的 TCP 对端——上一跳，从不被改写，不可伪造，L4 和 L7
  都可用。`RemoteIp` 是经过 `X-Forwarded-For` 解析、并对照可信代理集合确认后的客户端——**仅限
  L7**；在 L4 的 `GeoIpLocation` 配置中使用 `RemoteIp` 会被判定为硬性接入错误，因为 L4 没有 HTTP
  头可供解析转发链。参见[`RemoteIp` 可被伪造](#remoteip-可被伪造directpeerip-不可以)。
- **`key`**：`cidr` 直接把地址本身与一个或多个 CIDR/裸 IP 做匹配（裸 IP 是其 `/32`/`/128`
  的简写），完全不需要 GeoIP 数据库——见[纯 CIDR 规则](#纯-cidr-规则永不触碰-failopen)。
  `country`/`continent`/`subdivision` 是 ISO 代码，编译期归一化为**大写**；未识别的代码只会
  产生 **WARN**，不会被拒绝（宽松接入策略）。`city` 是自由文本，匹配 `names["en"]`，区分大小写
  ——见[`city` 只匹配英文名称](#city-只匹配英文名称)。
- **`values`**：必须**非空**——空列表在入库时即被拒绝。一条没有取值的规则不表达任何判据，
  若按"匹配一切"解读，会让 deny 组变成全站拒绝、让 allow 组变成敞开的白名单。要去掉一个
  判据，请删除该条规则。一个记录无法提供的维度会让规则直接**不匹配**——没有单独的
  `onMissing` 开关可配置；见[缺失维度](#缺失维度只是普通的规则未命中)。

### 未知字段会被拒绝，而不是忽略

`GeoIpLocation` 的每个配置结构体都是严格的：本插件不认识的字段名会导致入库直接失败并指出是哪个
键，而不是静默忽略这一行。

这是刻意为之，比大多数插件都严。geo 配置是一项安全控制，而它的各种拼写错误全都会**向放行的
方向**失效：

| 拼错 | 加严之前的后果 |
|---|---|
| `denyRuleGroup:`（少了 s） | 整个规则列表消失，配置变空——也就是只解析模式。所有请求全部放行，`failOpen: false` 也不再是必填，而资源状态依然是 `Accepted: True`。 |
| 规则里的 `valuez:` | `values` 落到空列表，而空列表当时意味着"匹配一切"——在 deny 组里是全站拒绝，在 allow 组里是敞开的白名单。 |

这两种情况从外部都看不出来：资源被接受，流量照常。EdgionPlugins 的 CRD 把插件 `config` 建模为
`x-kubernetes-preserve-unknown-fields`，API server 会原样转发未知键，因此这里的 schema 严格性是
唯一能拦住它们的关口。

实际影响是：**空配置 `config: {}` 现在只可能因为有人真的这么写，**而绝不会是某条规则悄悄没解析
成功。

### 规则分组：组内 AND，组间 OR

```yaml
denyRuleGroups:
  - name: cn-beijing
    rules:
      - source: DirectPeerIp
        key: country
        values: ["CN"]
      - source: DirectPeerIp
        key: city
        values: ["Beijing"]
```

一个分组内的每条规则都是 AND 关系；`denyRuleGroups`/`allowRuleGroups` 各自内部是 OR
关系。插件的判定逻辑（`decide()`）：

1. 任意一个 `denyRuleGroups` 分组命中 → **拒绝**。
2. `allowRuleGroups` 非空且没有分组命中 → **拒绝**（白名单未命中语义）。
3. 否则 → `defaultAction`。

**请始终在同一个 AND 分组内将 `city` 或 `subdivision` 与 `country` 配对使用。** 两者都不是
全局唯一的："Cambridge" 在多个国家都存在，单独的 `subdivision: ["CA"]` 会同时匹配加利福尼亚州
和任何代码为 `CA` 的加拿大省份。

**L4 没有专门的 `city` 限制，但按惯例 city 规则只用于 HTTP 层。** 在连接这一层没有请求属性可供
组合，因此细粒度的地理规则既难以运维又容易过度拦截；城市级别的判定应该放在 HTTP 层。

### `failOpen`——数据库不可用与查询失败时的处置方式

当 `denyRuleGroups`/`allowRuleGroups` 非空（规则模式）时，`failOpen` 是**必填**的；在只解析模式
下则会被**拒绝**——控制器接入路径和 Gateway 构造路径运行的是同一套校验，因此两者永远不会产生
分歧。它**只管这个插件自己的判定**，当 GeoIP 数据库本身不可用（`GeoLookup::Unavailable`）或已
加载数据库的查询失败（`GeoLookup::Failed`）时生效，绝不影响链上其它插件——这种隔离性正是把地理
能力独立成一个插件、而不是把谓词内嵌进另一个插件配置的核心理由。

| 情况 | 由谁决定 |
|---|---|
| 完全没有加载数据库（`GeoLookup::Unavailable`） | `failOpen` |
| 数据库已加载，但该地址无法解码（或 reader panic）（`GeoLookup::Failed`） | `failOpen` |
| 数据库已加载，但没有该地址的记录，或记录缺少该维度 | 普通的规则未命中（缺失的维度不会匹配一条有约束的规则） |

- **`failOpen: false`** —— **HTTP**：插件拒绝该请求（`403`，JSON 正文）。**L4**：连接被拒绝，
  且 `edgion_geoip_denied_total` 计数加一：无数据库时 `reason="db_unavailable"`，已加载数据库但
  本次查询失败时 `reason="lookup_failed"`。
- **`failOpen: true`** —— 插件**不表态**：HTTP 会让请求原样通过；L4 会让连接继续交给下一个
  Stream 插件。

仅含 IPv4（或覆盖不完整）的数据库对原生 IPv6 地址返回 MMDB 输入错误时，结果是 `Failed` 而不是
`NotFound`，因此走 `failOpen`，而不是普通的规则未命中。官方 GeoLite2 一类的库包含 IPv6。
IPv4-mapped 对端会在查询前规范化成 IPv4。

**每种处置方式在冷启动窗口期间具体的代价**（刚发布、`emptyDir` 上还没有数据库的时期，或任何更新器
一次都没有成功过的时间段）：

- 在安全相关规则（例如"拦截这个国家"）上使用 `failOpen: false`，意味着在数据库加载完成前，所有
  命中该规则分组的请求都会被**拒绝**——你用可用性换取了"该规则从未被静默绕过"的保证。
- `failOpen: true` 意味着在数据库加载完成前，所有请求都会被**放行**，就像 `GeoIpLocation`
  根本不在链上一样——你用执行力换取了这段窗口期的可用性。

没有第三种选项，也没有默认值；只要配置了任何规则分组就必须显式写明，而在只解析模式下不能写它。

#### 纯 CIDR 规则永不触碰 `failOpen`

完全由 `key: cidr` 规则组成的规则集完全不需要 GeoIP 数据库——CIDR 匹配直接比较对端地址，无需
MMDB 查询。HTTP 和 L4 插件都会按来源（`source`）检测这一点（`source_needs_geo_facts`），当某个
来源的所有规则都不需要解析出的维度时，直接跳过数据库查询，因此一份纯 IP 允许/拒绝列表永远不会
触碰 `failOpen`——无论那个不相关的国家数据库是否加载，它都保持正确。

### `RemoteIp` 可被伪造；`DirectPeerIp` 不可以

`RemoteIp` 的解析地址**来自 `X-Forwarded-For`**，属于客户端可控输入。只有当部署真正终结在可信
代理之后——即配置了 `RealIp` 插件，且其 `trustedIps` 集合确实精确匹配你真实的代理/CDN 出口
范围，同时连接的 TCP 对端也在该集合内——它才是可信的。如果这些条件不满足，`RemoteIp` 就只是
TCP 对端地址的原样拷贝，`RemoteIp` 规则会（无害地）退化为与 `DirectPeerIp` 规则读取相同的值。

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
`1.1.1.1` 所在的地理位置，从而完全绕过一条来源为 `RemoteIp` 的拒绝规则。这是一个在实践中很可能
发生的误配置——某个 CDN 发布的出口范围不完整或经常变化，运维方"为了保险"扩大了 `trustedIps`，
却在不知不觉中把地理判定的完全控制权交给了每一个客户端。

> **关于自动检测的说明。** 目前**没有**任何接入时的校验会将 `RemoteIp` 的 `GeoIpLocation` 规则
> 与所在监听器的 `RealIp`/`trustedIps` 配置做交叉检查。`RemoteIp` 是否可信完全需要运维方自行核对
> 同一路由/网关上的 `RealIp` 插件配置来确认。如果你的部署并没有真正终结在一个可信、范围精确的
> 代理之后，任何关乎安全的规则都应优先使用 `DirectPeerIp`——它总是读取原始 TCP 对端地址，不会受
> 请求头影响。L4 的 `GeoIpLocation` 配置会直接拒绝 `RemoteIp`，所以这个问题只与 HTTP 层有关。

### 没有 `registered_country` 兜底

当 `country.iso_code` 缺失时，刻意不回退到 `registered_country` 或 `represented_country`。
卫星 ISP、anycast 地址段、部分匿名化网络，以及某些军事/使馆分配段，都存在已填充的
`registered_country`（**注册方**）但无法解析出 `country`（**客户端的真实位置**）——如果混用
两者，会导致同一条 `country` 规则对不同客户端表达出不同的含义。这些客户端会解析为
`country: None`，一条有约束的 `country` 规则对它们永远不会命中。如果你的流量中卫星或 anycast
客户端占比较高，预期它们中的一部分会按设计永久无法解析，而不是遇到 bug。

其他形式的缺失也是同样处理：该地址在 MMDB 中完全没有记录，以及记录存在但缺少这一具体维度（对
`subdivision`/`city` 来说非常常见——很多携带 city/subdivision 数据的记录只填充 `geoname_id` +
名称，没有 `iso_code`，`city.names` 对不常见的地方也经常缺少 `"en"` 键），二者都会直接让一条
有约束的规则不匹配。

### `city` 只匹配英文名称

`city` 取自 `city.names["en"]`——英文本地化名称，永远不是翻译后的名称。用
`values: ["北京"]`（或任何非英文拼写）写的城市规则永远不会匹配，因为查询结果始终是英文形式。
`city` 和 `subdivision` 也**不是全局唯一的**——"Cambridge" 在多个国家都存在，单独的
`subdivision: ["CA"]` 会同时匹配加利福尼亚州和任何代码为 `CA` 的加拿大省份。请始终在同一个 AND
分组内将 `city` 或 `subdivision` 与 `country` 配对使用（见上方规则分组示例）。

### 缺失维度只是普通的规则未命中

无论由哪个产品构建，解析出的记录都是统一的。当记录携带该数据时 `city`/`subdivision` 是
`Some(...)`，不携带时是 `None`——原因可能是以下三种之一（数据库从不携带该维度、这一条具体记录
缺少该维度、或该地址根本没有解析出记录），这三者是无法区分的。它们都只是普通的"这条有约束的规则
不匹配"——没有像其他插件那样单独的缺失处理开关（`onMissing`）；上面[规则形状](#规则形状)一节里
"`values` 必须非空 / 维度缺失即不匹配"的规则已经覆盖了这一切。

**不存在"数据库分级"这个概念。** 该特性早期草案曾把 MaxMind 的产品线划分为"City 级"与
"Country 级"，并在一条 `city`/`subdivision` 规则针对看起来像"Country 级"的数据库编译时发出
警告。这一检查已被移除：`database_type` 只是一段自由文本（`GeoLite2-City`、
`GeoIP2-Enterprise`，或某个供应商自己的 OEM 名称……），并没有可靠的办法把任意字符串归类为"有
city 数据"还是"没有"——一个确实携带 city/subdivision 粒度、但其 `database_type` 字符串不包含
可识别关键字的数据库，会触发一条**错误**的警告，告诉运维方一条实际生效的规则是失效的，这比完全
不警告更糟。这里没有任何接入时或 Gateway 日志级别的信号，因为 Gateway 侧根本没有可校验的东西：
一条 `city`/`subdivision` 规则是否最终能解析出值，完全取决于运维方加载了什么数据库，而这在每一次
更新周期都可能发生变化。请查看访问日志中的 `geo` 块，确认当前加载的数据库实际能解析出哪些字段，
并据此调整你的规则分组。

## L4（TCP/TLS）注册

`GeoIpLocation` 作为 `EdgionStreamPlugins.spec.plugins` 条目注册——**仅限**第一阶段
（`ConnectionFilter`，TLS 握手之前）。它没有被加入 `tlsRoutePlugins`（第二阶段），该阶段目前
仍然只允许 `IpRestriction`。

L4 的配置结构与 HTTP 完全相同，但有两条特定于本层的准入规则：

| 规则 | 原因 |
|---|---|
| 拒绝 `source: RemoteIp` | L4 没有 `X-Forwarded-For` 可用于解析真实客户端。请使用 `DirectPeerIp`。 |
| **拒绝只解析模式（没有任何规则分组）** | L4 没有访问日志，`StreamContext` 也不携带任何日志出口，解析出来的地理信息无处可去——这个插件将确定无疑地什么也不做。L4 条目必须至少携带一个 `denyRuleGroups`/`allowRuleGroups` 分组。 |

第二条正是"只观察不拦截"属于 **HTTP 层能力**的原因：在 L7，只解析条目会填充访问日志的 `geo`
块，这是一项实实在在的产出；而在 L4，同样的配置会变成一项静默不运行的安全控制，因此在准入时
直接拒绝，而不是当作一个有文档记载的空操作接受下来。

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: geo-block
  namespace: production
spec:
  plugins:
    - type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: block-ru
            rules:
              - source: DirectPeerIp
                key: country
                values: ["RU"]
```

`source: RemoteIp` 在 L4 配置中是硬性接入错误——在控制器接入和 Gateway 构造两条路径上会被
一致地拒绝（两者调用的是同一个共享校验函数），因为 L4 没有 `X-Forwarded-For` 可用来解析真实
客户端。

### 永远是传输层对端——地理判定没有 PROXY protocol 的顾虑

由于 `GeoIpLocation` 的 L4 注册仅在第一阶段生效，它看到的永远是原始传输层对端：PROXY protocol
的解析严格发生在 `accept()` **之后**，所以第一阶段的连接过滤器永远看不到 PROXY 解析后的客户端。
这比 `GlobalConnectionIpRestriction` 自身的 IP 规则要简单——那些规则会在全部三个 Stream 阶段
运行，因此在第一阶段看到的是负载均衡器的地址，而在第二/三阶段看到的是 PROXY 解析后的客户端地址；
`GeoIpLocation` 的 `DirectPeerIp` 来源没有这种歧义需要说明，因为它只会在这一个阶段运行。

### 无就绪门控：运维方自行承担冷启动窗口

一次刚发布、还没有数据库的部署（`emptyDir` 上很典型）**不会**扣住就绪探针。`failOpen` 就是用来
声明应该发生什么的：`true` 接受一段地理策略未生效的窗口期，`false` 接受一段流量被拒绝的窗口期。
首次加载前的退避（见下文）会把这段窗口限制在几秒钟以内，而不是完整的 `retry_interval_sec`；
`edgion_geoip_db_loaded` 是用来告警"这个状态是否还在持续"的指标。

### L4 地理拒绝永远不会被记录日志，也不携带客户端地址

一次 L4 地理拒绝**在任何阶段都不会产生日志条目**，也**不会在任何地方记录客户端地址**。唯一的信号
是计数器 `edgion_geoip_denied_total{layer,reason}`（见[指标](#指标)）。这是刻意为之的设计：
pingora 会在其 accept 循环内直接丢弃第一阶段拒绝的连接，所以那里确实无法产生日志条目，而且在连接
洪泛攻击下，逐连接的拒绝日志本身也是一个放大向量——统一的、仅计数的信号比逐连接细节更有价值。
HTTP 层的地理拒绝不受此规则影响——它们保留正常的 `EdgionErrorCode` 诊断和访问日志记录，因为那里本来
就存在逐请求的日志。

## 访问日志

没有全局开关：地理能力完全由插件驱动，访问日志只会报告本次请求中 HTTP `GeoIpLocation` 插件确实
查询过的槽位。在一条路由的 `requestPlugins` 里加入 `GeoIpLocation` 条目——无论是只解析模式还是
规则模式——都会让命中的请求的访问日志条目新增一个 `geo` 对象，其中只包含被查询过的槽位：

```json
"geo": {
  "remote": { "status": "found", "country": "CN", "continent": "AS", "city": "Beijing" },
  "direct_peer": { "status": "found", "country": "US" }
}
```

每个槽位（`remote` = `RemoteIp` 侧解析结果，`direct_peer` = `DirectPeerIp` 侧解析结果）都是一个
独立的、逐请求的 `OnceLock`。访问日志只读取插件在本次请求中已经解析过的槽位——它本身从不触发
查询。因此完全没有 `GeoIpLocation` 条目的路由不会产生任何 MMDB 查询。只解析模式（`config: {}`）
永远会同时解析两个槽位——这正是它存在的全部意义；规则模式只会解析已配置规则实际用到的来源/维度
（例如，一份纯 CIDR 规则集，对它覆盖的来源根本不需要解析出任何维度）。每个槽位带有有界的
`status`：`found`、`not_found`、`unavailable` 或 `lookup_failed`。`lookup_failed` 表示数据库已
加载，但该地址无法解码（或 reader panic）；其中不会包含解码器原文、IP 或数据库路径。

### 坐标

经纬度没有开关：只要解析出的记录确实携带坐标，`geo` 槽位就会输出 `coord`；不携带时则不输出——
与 `geo` 对象其余部分"只读取插件已经解析过的内容"是同一条规则，没有额外的门控。存在时，坐标会以
单个 `coord` 字符串形式输出，并经过与 `direct_peer_ip`/`remote_ip` 相同的脱敏处理。

**`coord` 没有按 key 单独解除脱敏的机制。** 脱敏完全由进程级的敏感日志模式控制——一个
全有或全无的开关。允许部分 ext 日志字段选择不脱敏的按 `LogExtSource` 白名单机制在这里不适用；
`direct_peer_ip` 和 `remote_ip` 今天也无法单独解除脱敏，`coord` 遵循同样的规则。

## 指标

| 指标 | 类型 | 标签 | 含义 |
|---|---|---|---|
| `edgion_geoip_db_loaded` | Gauge | 无 | 已加载 GeoIP 数据库时为 `1`，否则为 `0`。在配置了地理规则的情况下对 `== 0` 告警——这是"安全控制项未在运行"的首要信号。 |
| `edgion_geoip_db_build_epoch_seconds` | Gauge | 无 | 已加载数据库的构建时间戳。对陈旧程度告警。 |
| `edgion_geoip_update_failed_total` | Counter | `reason`（`fetch` \| `encoding` \| `size_cap` \| `stall` \| `validation` \| `rename` \| `disk` \| `deadline`） | 按分类统计的后台**更新周期**失败。`disk` 表示磁盘空间预检查拒绝启动（还没有发出任何请求）；`deadline` 表示整个周期超过了 `download_deadline_sec`（即使在持续取得进展），区别于 `stall`（在 `stall_timeout_sec` 内没有任何进展）——两者对应相反的修复方式。 |
| `edgion_geoip_lookup_panic_total` | Counter | 无 | 一次逐请求的 MMDB 查询发生 panic（已捕获）并报告为 `Failed`。与上面的 `edgion_geoip_update_failed_total` 不同：这是查询路径上的逐请求事件，不是更新周期失败。不含 panic 的解码失败共享 `Failed` 结果，但不增加本计数器。 |
| `edgion_geoip_denied_total` | Counter | `layer`（`stage1` \| `tcp` \| `tls`）、`reason`（`deny_country` \| `allow_miss` \| `db_unavailable` \| `lookup_failed`） | 被 `GeoIpLocation` 拒绝的 L4 连接。在当前的接入方式下 `layer` 永远是 `stage1`，因为那是该插件唯一的 L4 注册点；另外两个标签取值是为了未来兼容而保留的。永远不携带客户端地址；HTTP 拒绝刻意不计入此处——它们已经出现在访问日志中。 |

## 多副本更新行为、抖动与 `.meta` 校验器

只有当本地校验器上下文满足条件时，更新器的请求周期才发送条件 `GET`（`If-None-Match` /
`If-Modified-Since`）。有效的 `304` 会休眠到下一个间隔且不触碰磁盘；不可信的 `304` 按 `fetch`
失败处理：清空校验器、使用冷启动退避，并让下一次请求保持无条件。`Content-Encoding` 不是
`identity` 的 `200` 响应会被直接拒绝（更新器不做解压——请让 `update_url` 指向一个纯 MMDB 文件，
而不是 `.mmdb.gz`）；`identity` 编码的 `200` 响应在
`max_db_bytes`/`stall_timeout_sec`/`download_deadline_sec` 的约束下流式写入
`<db_path>.tmp`；成功后 `rename` → 重新 mmap → 热替换 → 将校验器持久化到 `<db_path>.meta`。

**退避分两种模式。** 在**首次**成功加载之前，重试使用较短的指数退避（1 秒起，倍增，上限 30 秒）——
`retry_interval_sec` 默认的 600 秒是为稳态调优的，如果用于首次加载，会让一个冷启动的 Pod 在实际上
不受保护的状态下持续十分钟。首次成功之后，普通失败使用 `retry_interval_sec` 加抖动。

**抖动同样作用于首次拉取，而不只是重试。** 如果没有抖动，200 个 Pod 的滚动重启会向镜像同时发出
200 个无条件的 GET 请求。不过光有抖动并不能让这些请求变成条件请求——这是下面 `.meta` 文件的
职责。

**`<db_path>.meta` 持久化 ETag/Last-Modified 校验器，用于生成条件请求。** 只有当 `db_path`
仍是普通文件且 provider 已加载活动数据库时，校验器才有资格使用。如果两者均可用，但 `.meta` 缺失
或无法读取，更新器会用数据库文件的 mtime 填充 `If-Modified-Since`。一个语法有效的空 sidecar
会刻意抑制这一兜底，让下一次请求保持无条件。

如果持久文件或活动 provider 任意一项缺失，更新器会在构造请求前同时清空内存校验器和 sidecar，
随后发出无条件请求。只有当本次请求确实携带校验器、且响应到达时上述两项本地条件仍成立，`304` 才会
被接受。其他 `304` 都按有界的 `fetch` 失败处理：清空校验器、进入冷启动退避，下一次请求保持无条件。
因此，仅丢失 `.meta`、但可用数据库仍存在时，会发出基于 mtime 的条件请求；丢失可用的本地数据库状态
时，才会发出无条件请求。

## 故障排查

| 现象 | 可能原因 | 解决方法 |
|---|---|---|
| `edgion_geoip_db_loaded` 一直是 `0`，但你的 YAML 明明设置了 `geoip.db_path` | `geoip:` 写在了文档根层级，而不是嵌套在 `gateway:` 下 | 把它移到 `gateway:` 下——见[最容易踩的坑](#-最容易踩的坑) |
| 配置加载失败，报错提到某个 `geoip` 子字段是未知字段 | `geoip` 块内部的 camelCase 拼写错误（例如 `dbPath`） | 使用 snake_case——`db_path`、`update_url` 等 |
| 全新部署之后 `edgion_geoip_db_loaded` 一直是 `0` | 首次启动时的正常状态——没有内置数据库，且更新器尚未完成第一个周期 | 等待首次成功周期，或检查 `edgion_geoip_update_failed_total{reason}` 是否卡在某个失败原因上 |
| Gateway 进程在没有 panic 日志的情况下崩溃，且发生在某个数据库文件刚被外部修改之后不久 | 有东西原地写入了 `db_path`，而不是用重命名做替换——SIGBUS | 停止任何原地写入的行为；只有内置更新器，或同文件系统上"临时文件+重命名"的写入方式被支持 |
| 更新周期以 `reason="disk"`（或类似 `ENOSPC`）持续失败 | 更新卷容量不足以完成一次交换（需要 `max_db_bytes` 的 `.tmp`，加上当前数据库大小 2 倍的旧 inode/rename 双持有开销） | 按上面的公式扩容卷，而不是扩到 `max_db_bytes` 的 3 倍 |
| 一个 `country` + `city` 规则分组永远不匹配任何内容 | 已加载数据库的记录不携带这条规则所需的 city 数据——见[不存在"数据库分级"这个概念](#缺失维度只是普通的规则未命中) | 查看访问日志的 `geo` 块并对照供应商文档，确认已加载的数据库实际能解析出哪些字段；如果 `city`/`subdivision` 规则在结构上永远不可能匹配，就去掉它 |
| 带翻译语言取值的 `city` 规则永远不匹配 | `city` 只会与 `names["en"]` 比较 | 使用英文城市名 |
| 黑名单模式下、来源为 `RemoteIp` 的规则被任意客户端轻易绕过 | `RealIp` 的 `trustedIps` 配得过宽（例如 `0.0.0.0/0`）——`RemoteIp` 退化为攻击者可控输入 | 将 `trustedIps` 收窄到你真实的代理出口范围，或改用 `DirectPeerIp` |
| L4 配置在接入/编译时被拒绝，报错提到 `RemoteIp` | L4 的 `GeoIpLocation` 配置中出现 `source: RemoteIp` 是硬性接入错误 | 改用 `source: DirectPeerIp`——L4 没有 `X-Forwarded-For` 可用来解析真实客户端 |
| 发生了一次 L4 地理拒绝，但任何地方都找不到对应的日志行 | 符合预期——L4 地理拒绝按设计永远不会在任何阶段被记录日志 | 使用 `edgion_geoip_denied_total{layer,reason}` 作为唯一信号 |
| "拦截这个国家，除非在办公室白名单里"这类规则表现不符合预期 | 检查 `IpRestriction`/其他准入插件相对 `GeoIpLocation` 在插件列表中的顺序——组合方式是按链上顺序生效，而不是靠单一合并插件 | 调整插件列表条目的顺序；没有可配置的跨插件豁免语义 |

## 相关文档

- [Gateway 配置](../configuration/gateway-config.md) —— 完整的 `GatewayConfig`
  schema 参考，包括 `geoip`
- [Real IP](../../user-guide/edgion-plugins/real-ip.md) —— 决定 `RemoteIp` 是否可信的
  `trustedIps` 配置
- [指标](../observability/metrics-inventory.md) —— 完整的指标清单，包括五个
  `edgion_geoip_*` 系列
