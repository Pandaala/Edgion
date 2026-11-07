# RegionRoute 区域路由

RegionRoute 通过请求字段、哈希和有序规则选择区域。插件必须以 `clusterRegionRef`
引用 `EdgionConfigData` 的 `ClusterRegionSetting` 变体，不新增 CRD kind。

## 配置归属

设置包含必填的 `clusterName`、`regionName` 和完整 `regions` 表。`regionName`
是本地区域的唯一来源，必须对应表内一行。Cluster 和 Region 名称区分大小写、
不进行归一化，不能为空白、不能包含控制字符，最多 63 个 UTF-8 字节。
插件只保留请求提取、哈希、路由规则和异常策略。

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: local-region
  namespace: production
spec:
  enable: true
  data:
    type: ClusterRegionSetting
    config:
      clusterName: production
      regionName: us-east
      regions:
        - name: us-east
          hashRange: [0, 499]
          backendEndpoint: api.us-east.example.com:443
          tls: true
        - name: eu-west
          hashRange: [500, 999]
          backendEndpoint: api.eu-west.example.com:443
          tls: true
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: regional-routing
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        clusterRegionRef:
          name: local-region
        failOpen: true
        statusOnError: 503
        keyGet:
          - type: header
            name: X-Account-Id
        hashCalc:
          algorithm: crc32
          modulo: 1000
        routeRules:
          - type: RouteByDirectRegion
            keyFrom:
              - type: header
                name: X-Region
          - type: RouteByHashRange
```

引用默认使用插件所在命名空间。启用引用授权校验时，跨命名空间引用需要目标命名空间的 ReferenceGrant，
但目标设置 `spec.visibility: Cluster` 时不需要授权对象。
旧变体和旧插件字段不提供兼容别名。

## 字段与路由

| 插件字段 | 默认值 | 含义 |
| --- | --- | --- |
| `clusterRegionRef` | 必填 | 设置名称及可选命名空间 |
| `failOpen` | `true` | 配置不可用时继续后续插件和正常代理 |
| `statusOnError` | `503` | failOpen=false 时返回的 HTTP 状态码，范围 400–599 |
| `keyGet` | `[]` | 顺序读取的共享请求键来源 |
| `hashKeyGet` | 无 | 预计算哈希来源 |
| `hashCalc` | 无 | 算法、模数和 writeBack 目标 |
| `routeRules` | `[]` | 有序路由规则 |
| `routeByKeyConfMatch` | 无 | 共享 matchMap 和 bypassClusterKeys |

规则按顺序执行。`RouteByDirectRegion` 使用自己的 keyFrom 查找已知区域。
`RouteByKeyConfMatch` 使用共享 keyGet：非空共享 matchMap 优先，bypassClusterKeys
中的键使用规则自己的映射。`RouteByHashRange` 是终止规则，即使没有匹配也不继续后续规则。

哈希路由优先使用 hashKeyGet 中位于 `[0, modulo)` 的整数；否则在配置了 hashCalc
时计算 CRC32 或 SHA-256，再写入 writeBack。算法默认为 crc32，模数默认为 1000，
必须至少为 2。未配置 hashCalc 时，表边界和预计算哈希仍使用模数 1000。
响应头写回必须符合共享响应头策略。

区域行保留 name、hashRange、backendEndpoint、tls 和 failoverTo。
表必须非空，名称非空且唯一，闭区间满足 `0 <= low <= high < modulo` 且互不重叠。
允许空隙，落入空隙的请求正常代理。端点支持主机名、IPv4 或方括号 IPv6，
可选端口为 1–65535，不接受 URL 或空主机。tls 默认 true；缺省端口为 TLS 的 443
或非 TLS 的 80。配置校验不绕过 DNS 解析后的地址安全检查。
主机名通过 Gateway 的后端 DNS 应答缓存解析。解析器和地址族依次取自 Gateway 的
`edgion.io/dns-resolver` / `edgion.io/dns-resolver-ref` / `edgion.io/dns-lookup-family`
注解，以及 EdgionGatewayConfig `dnsResolver` 默认值；默认地址族为 `ipv4Only`，只有
IPv6 记录的主机名需要配置 `ipv6Only` 或 `all`。解析失败会缓存 1–10 秒。IP 字面量按原样使用。从轮转的起始位置开始，连接第一个通过检查的地址。

远程路由保留原始 Host、SNI、路径和请求体。目标等于设置中的 regionName 时，
使用正常本地上游。failoverTo 必须指向另一行，不允许自引用和循环；执行时只跳一层，
例如 `a -> b -> c` 中选中 a 的请求实际路由到 b。

## 配置异常与动态恢复

设置缺失、禁用、引用被拒绝、类型不匹配、区域表无效，或消费者哈希配置不可用时，
插件在读取请求键、写回哈希和设置上游之前执行异常策略。默认 failOpen=true
完整跳过当前插件，继续后续插件和正常代理；failOpen=false 按 statusOnError 返回错误。
不保留基础表兜底，也不保留上一次有效表。空规则仍先检查设置可用性，再不执行路由。

策略只处理可以解析的配置不可用。YAML 无法解析、策略字段类型错误、必填字段缺失、
结构安全校验失败，不会被转换为宽松放行。Controller 报告配置和引用状态，Gateway
独立执行运行时约束。

设置原文与校验证明原子发布；每个请求使用同一代 regionName 和 regions。
完整表校验发生于配置更新，不在请求热路径重复执行。修复设置后可动态恢复，无需重建插件。
配置已持久化、Controller 已观察、Gateway 已应用是不同状态。

## 诊断与染色

插件访问日志记录 setting_reference_unavailable、setting_missing、setting_disabled、
setting_type_mismatch、setting_invalid 或 hash_modulo_invalid。
计数器 `edgion_region_route_config_events_total` 使用这些固定原因标签。

[染色](dye.md) 默认关闭。成功选择区域时可记录最终区域名，包括本地透传。
配置不可用时，failOpen=true 对应 fallback，否则对应 reject，原因为
configuration-unavailable。成功能力要求规则非空；没有匹配或上游跳转已被其他插件占用时，
不记录选择结果。显式启用染色后仍遵守共享防伪和请求头处理策略。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: regional-routing
  namespace: production
spec:
  requestPlugins:
    - type: RegionRoute
      dye:
        request:
          - name: X-Served-Region     # value omitted => final selected region (on: [success] by default)
        response:
          - name: X-Edgion-Outcome
            on: [fallback]            # value omitted => region_route:fallback:configuration-unavailable
      config:
        clusterRegionRef:
          name: local-region
        failOpen: true
        keyGet:
          - type: header
            name: X-Account-Id
        routeRules:
          - type: RouteByHashRange
```

## 原始资源浏览与单设置故障切换

Controller 提供以下源资源接口：

- `GET /api/v1/cluster-region-settings/keys?limit=10&continue=...` 仅返回资源定位与
  Cluster/Region 身份。匹配结果为空时仍可能存在续页游标。禁用设置及区域表无效但身份可读的
  设置仍可发现；区域归属来自 `regionName`。
- `GET /api/v1/cluster-region-settings/{namespace}/{name}` 每次重新读取原始源对象，
  保留未知字段，不返回加工后的 Store 对象。
- `GET /api/v1/cluster-region-settings/{namespace}/{name}/consumers` 使用源区域表和
  Controller 已观察到的消费者引用计算兼容性。`compatibleWithTable` 不是 Gateway 应用确认；
  `settingEnabled`、`tableObservation`、`consumerObservation` 分别说明这些输入。

单目标操作先向 `/api/v1/cluster-region-settings/{namespace}/{name}/failover/preview`
POST `{regionName, failoverTo}`，空 `failoverTo` 表示清除目标。预览返回变更、消费者兼容性及
源 `precondition`。将相同目标字段和该 precondition 提交到
`/api/v1/cluster-region-settings/{namespace}/{name}/failover`。界面修改目标后应使预览失效。

precondition 绑定规范化源内容、UID、resourceVersion 和未知字段，是内容相等条件，不是授权
凭据或意图签名。源已变化时返回 409，需要重新预览。状态已匹配时返回 `modified: false`，不
重复写入。操作不会自动创建、启用或修复缺失、禁用或无效设置。完整的新区域表必须有效；已有的
消费者特定异常单独展示，不新增所有消费者必须健康的全局门禁。

写入成功仅报告 `persistence: persisted`，无需变更时为 `unchanged`；Controller 观察和
Gateway 应用仍标记 `not-confirmed`。超时可能发生在持久化之后，应先回读再重新预览，不要自动
重放。文件模式保证同进程修改持锁到实际文件操作完成，包括取消场景，但不承诺与外部文件编辑
之间的原子 CAS。Kubernetes 使用原生 resourceVersion 条件替换，etcd 使用修改 revision 事务。
可靠多目标操作不属于此单设置接口范围。
