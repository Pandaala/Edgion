# OpenResty LRU Analysis

## Goal

分析 OpenResty `lua-resty-lrucache` 的源码实现，识别哪些部分适合 Edgion 借鉴，哪些部分不适合直接照搬。

## Scope

- 下载官方源码到 `vendor/lua-resty-lrucache/`
- 阅读 `resty.lrucache` 与 `resty.lrucache.pureffi`
- 提炼可借鉴设计点
- 明确不建议照搬的部分

## Steps

- `completed` `step-01-source-and-takeaways.md`

