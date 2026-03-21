# Step 03 - Validation

## Tests

执行：

- `cargo test core::gateway::cache::lru::tests:: --lib`

结果：

- 6 个新增测试全部通过

覆盖点：

- 基本插入与命中
- TTL 过期
- LRU 淘汰
- 更新已有 key 时刷新值、TTL 与顺序
- 零 TTL 删除
- remove / clear

## Residual Risks

- 还没有并发压测
- 还没有接入具体插件验证真实使用路径
- 还没有加入 singleflight，外部 lookup 场景后续仍可能需要补

