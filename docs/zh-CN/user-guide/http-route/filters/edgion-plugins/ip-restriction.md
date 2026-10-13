# IpRestriction

`IpRestriction` 通过有序的 `allowRefs` / `denyRefs` 引用 `EdgionConfigData` 的 `IpList`，决定是否允许请求。所有 IP/CIDR 内容都写在 IpList 的具名 `IpGroup` 中，由 Gateway 集中编译；插件不保存或展开 CIDR。

## 配置示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: office-networks
  namespace: edgion-default
spec:
  data:
    type: IpList
    config:
      items:
        - name: office
          cidrs: ["192.0.2.0/24", "2001:db8::/32"]
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: blocked-networks
  namespace: edgion-default
spec:
  data:
    type: IpList
    config:
      items:
        - name: blocked
          cidrs: ["192.0.2.100/32"]
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: office-access
  namespace: edgion-default
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        ipSource: RemoteIp
        allowRefs:
          - name: office-networks
        denyRefs:
          - name: blocked-networks
        defaultAction: deny
        status: 403
```

在 HTTPRoute 中使用 `ExtensionRef` 绑定上述 EdgionPlugins。一个 IpList 可以包含多个具名组，也可以由多个策略共享；需要独立管理时引用多个列表即可。

## 字段

| 字段 | 类型 | 默认值 | 含义 |
|---|---|---|---|
| `allowRefs` | EdgionConfigDataRef 对象数组 | 未配置 | 非空引用开启白名单模式。 |
| `denyRefs` | EdgionConfigDataRef 对象数组 | 未配置 | 有序黑名单来源，命中拒绝优先。 |
| `ipSource` | `RemoteIp` / `DirectPeerIp` | `RemoteIp` | 规范解析后的客户端地址 / 物理 TCP 对端。 |
| `defaultAction` | `allow` / `deny` | `allow` | 未启用白名单且未命中拒绝时的动作。 |
| `message` | 字符串 | 默认拒绝消息 | 拒绝响应 JSON 的 message。 |
| `status` | 200–599 整数 | 403 | 策略拒绝使用的最终 HTTP 状态。 |

`allowRefs` 与 `denyRefs` 至少一个非空。每条引用必须提供非空 `name`，可选 `namespace` 默认使用消费者所在命名空间；跨命名空间仍遵守 ReferenceGrant 和可见性规则。不要填写 Controller 的内部权限证明或拒绝标记。

旧 `allow` / `deny` 集合字段已移除，即使为 null、空数组或与新引用混用，也会形成无效策略。拒绝的原始值不会保留，但无效标记会经过配置传输。其他未知键仍按原有规则提示警告。

## 判定顺序

1. 按引用顺序检查可用 deny 列表，首个来源命中即拒绝。
2. 非空的已配置 allowRefs 保持白名单模式：按顺序命中则允许，否则拒绝。
3. 未启用白名单且未命中 deny 时使用 defaultAction。

同一个 IpList 内采用最长前缀确定具名组；后续列表里更窄的前缀不会覆盖较早来源的命中。deny 始终优先于 allow。

缺失、禁用、类型不符、无权限或资源身份不一致的引用会被略过，其他有效引用继续参与。allow 引用全部不可用时仍然是白名单，会拒绝所有未命中地址；仅有 deny 引用且全部不可用时仍使用 defaultAction，因此可能放行。ACL 引用不会自动变成“所有依赖必须可用”的门禁。

策略结构或权限证明无效时，运行时构造失败并封闭该代配置。普通共享 IP Condition 和 Geo CIDR 引用采用各自的必需依赖规则，缺失时由 HTTP 运行时预检返回配置/依赖错误，不作为 ACL 白名单未命中处理。

## 地址与列表

RemoteIp 使用规范 RealIp 结果，随后回退到有效传输对端。使用转发头前，应通过 RealIp 的 trustedIpRefs 配置可信代理。DirectPeerIp 始终读取物理对端，不受转发头和 PROXY 协议声明的来源影响。

IPv4、IPv6、IPv4 映射 IPv6 使用同一规范匹配器。IpList 中的裸地址按主机前缀处理。列表自身负责 partial-good 接纳：保留有效 CIDR，在列表的 PartiallyInvalid 状态报告无效项；可用但内容为空的 allow 匹配器仍然导致白名单未命中。

## 动态更新与发布

IpList 内容、启用状态、类型或可见性变化，会影响后续判定，无需重建引用插件。一次分派的 Condition 与 ACL 使用同一不可变 ConfigData 快照，嵌套 ExtensionRef 也遵守这一点；已持有快照保持原代，下一次分派可观察删除与恢复。

已经发布的 typed 无效策略代会替换旧可执行策略，形成封闭的配置结果；发布干净的新代后恢复。旧字段存在的捕获也属于此情况。若更新在 Kubernetes 接纳或 typed 解码阶段被拒绝，则没有进入发布，之前已接纳的一代仍保留。

## 响应和日志

普通策略拒绝返回配置的最终状态和包含 message 的 JSON，默认状态为 403，响应体在冷构造阶段预备。

ACL 日志保留命中组：deny:group=blocked 或 allow:group=office；白名单未命中记录 deny:not-in-allow，默认动作记录 allow:default / deny:default。这是 ACL 的明确诊断例外，不适用于凭证认证失败日志。

L4 IpRestriction、TLS-route IpRestriction 与 GCIR 的每个 profile 都只配置 allowRefs / denyRefs。GCIR 仍拥有具名标量策略与可选 Selector 动态选型，IP 集合通过 IpList 实时读取。参见 [Stream 插件](../../../tcp-route/stream-plugins.md)。
