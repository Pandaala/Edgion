# Edgion 内存泄漏专项 Review（第 1 轮）

本轮 review 目标是结合 `Edgion/skills` 中的架构、开发、可观测性、测试文档，对 `Edgion` 的常驻运行路径做一次偏“泄漏 / 无界增长 / 生命周期失控”方向的深度审查，而不是普通功能性 review。

## 本轮方法

本轮主要依据以下知识来建立审查模型：

- `skills/00-architecture/*`：识别 Controller / Gateway 的长生命周期组件、请求生命周期、插件运行时、gRPC 同步链路
- `skills/02-observability/*`：识别日志、metrics、gauge 对称性、热路径内存占用风险
- `skills/03-testing/*`：辅助判断哪些行为是长期运行服务路径，哪些只是测试代码

我优先排查了最容易产生内存问题的模式：

- 进程级全局单例、全局 map、全局索引
- `tokio::spawn` / 线程启动后没有回收或停止条件
- 有 TTL 但没有物理淘汰的缓存
- per-request / per-client / per-backend 维度可能无限增长的数据结构
- 有界 channel 外层再包一层 detached task，导致“队列有界但任务无界”
- reload / relink / leader 切换之后没有重置的全局状态

## 本轮结论摘要

### 已确认问题

1. `Gateway / UDP` 存在后台清理协程常驻导致 `Arc<EdgionUdp>` 无法释放的问题。
2. `Gateway / Auth` 中 `BasicAuth`、`LdapAuth` 的成功认证缓存只有“逻辑 TTL”，没有“物理淘汰”，会长期累积冷 key。
3. `Controller / Workqueue` 之前包了一层 detached `tokio::spawn`，高压时会在队列外堆积大量等待发送的任务。
4. `Controller / ReferenceGrant` 相关全局单例在 registry 清理时没有一并清空，存在跨 epoch 残留。
5. `Controller / 路由索引`（GatewayRouteIndex、AttachedRouteTracker）是进程级单例，但 reload / relink 后没有完整重建路径。

### 高风险疑点

1. `Gateway / OIDC introspection cache` 没有容量上限，TTL 只限制逻辑可用性，不限制峰值占用。
2. `Gateway / UDP` 在高基数源地址场景下，session / socket / task 没有明确上限。
3. `Gateway / LB` 的 per-backend 历史状态清理不完整，尤其 EWMA 全局表在生产路径里看不到稳定 remove。
4. `Controller / SecretStore`、`NamespaceStore` 是全局常驻缓存，但初始化阶段没有看到权威式全量替换被稳定调用。

## 文档索引

- `gateway/memory-leak-review.md`
- `controller/memory-leak-review.md`
- `appendix/false-positives.md`

## 说明

这是第 1 轮 review，重点是先把“最像内存泄漏、最值得先修”的问题钉住。后续如果继续做第 2 轮，建议再深挖：

- LinkSys provider 生命周期
- conf_sync watch / client registry 在异常断链下的残留行为
- 各 HTTP 插件内部缓存与连接复用策略
- 后端发现、健康检查、TLS / ACME 在长时间 churn 下的堆积风险
