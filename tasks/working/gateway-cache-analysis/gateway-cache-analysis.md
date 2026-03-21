# Gateway Cache Analysis

## Goal

整体分析 Edgion `gateway` 层代码，重点梳理插件体系中的 cache 需求、当前已实现的 cache 形态、缺口与适合的 cache 类型；同时补充外部 cache 参考，包括 Pingora cache、Nginx 类 proxy cache，以及 Rust 常见开源 cache 实现。

## Current Scope

- 分析 `src/core/gateway/` 的主要运行链路与现有 cache/store
- 分析全部 HTTP 插件、Gateway API filter adapter、stream 插件的 cache 需求
- 区分本地内存 cache、编译/预解析 cache、分布式状态 cache、内容 cache、请求体 cache 等类型
- 整理外部参考，便于后续设计 `EdgionCache` 或插件级 cache 能力

## Steps

- `completed` `step-01-gateway-overview-and-current-caches.md`
- `completed` `step-02-plugin-cache-needs-matrix.md`
- `completed` `step-03-external-cache-survey.md`

## Out Of Scope

- 本次不直接修改 gateway/cache 代码
- 本次不直接设计最终 CRD / API schema
- 本次不做性能压测与 benchmark
- 本次不做跨节点一致性方案定版

## Deliverables

- Gateway 层已有 cache / store 地图
- 插件 cache 需求矩阵
- 外部 cache 技术参考与选型建议

