# 非内存泄漏场景

## 辅助 LB Store 懒创建后的陈旧条目

适用位置：

- `src/core/gateway/backends/discovery/endpoint_slice/`
- `src/core/gateway/backends/discovery/endpoint/`

判定结论：

`RoundRobin`、`ConsistentHash`、`LeastConn`、`EWMA` 四类 LB store 中，某个 service 在历史上用过的可选算法 LB 实例，即使后续 route 改了 LB 策略，也可能继续留在对应 store 中。这个现象在 Edgion 当前设计下不视为内存泄漏。

简要原因：

- 这些实例始终由全局 store 强引用，属于可达的 runtime cache，不是失去引用却无法释放的对象
- 项目早期设计本来就是为每个 service 预建全部 LB；后续改为按需创建，只是优化创建时机，不改变单 service 的容量上界
- 单个 service 的此类实例数量有固定上界，最多对应 4 种算法，不会因策略反复切换而无限增长
- 当 service/endpoint 数据真正消失时，相关 LB 条目会随着共享数据层缺失而被删除

review 处理建议：

- 不要将该场景标记为“内存泄漏”或“高危 leak”
- 如需指出，可表述为“懒创建后的有界陈旧实例保留”或“可选优化项”
- 只有当设计目标明确变为“仅保留当前仍被 route 引用的算法实例”时，才应按行为偏差继续讨论
