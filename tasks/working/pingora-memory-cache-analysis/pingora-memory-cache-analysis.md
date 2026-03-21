# Pingora Memory Cache Analysis

## Goal

调研 `pingora-memory-cache` 是否比 `mini-moka` 更适合 Edgion 当前的“短 TTL 结果缓存”需求。

## Scope

- 下载 Pingora 仓库到 `vendor/pingora/`
- 分析 `pingora-memory-cache`、`tinyufo`、`pingora-timeout`
- 收集基础采用信号
- 和 `mini-moka` 做粗粒度对比

## Steps

- `completed` `step-01-source-shape-and-fit.md`

## Out Of Scope

- 本次不做 Edgion 接入 PoC
- 本次不做 benchmark

