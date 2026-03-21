# Mini Moka Analysis

## Goal

调研 `mini-moka` 是否适合 Edgion 的短 TTL 结果缓存需求，并判断：

- 是否值得直接引入
- 是否适合只借部分实现思路
- 是否值得直接拷一部分代码自用

## Scope

- 下载官方源码到 `vendor/mini-moka/`
- 盘点代码量、依赖、关键模块
- 收集一些公开可见的使用者线索
- 给出对 Edgion 的实用判断

## Steps

- `completed` `step-01-source-and-adoption.md`

## Out Of Scope

- 本次不改 Edgion 代码
- 本次不做完整 PoC
- 本次不和 `pingora-memory-cache` 做深度 API 对比实现

