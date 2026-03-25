# P8: ListenerSet

**工作量**: ~11 天
**测试**: 10 个 (全部 v1.5 新增)
**技术方案**: `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md` § P8

## 实现阶段

### Phase 1: 类型和注册 (~1 天)
- [ ] `src/types/resources/listener_set.rs`: 新文件
- [ ] `src/types/resources/gateway.rs`: 新增 `AllowedListeners`
- [ ] `src/types/resource/kind.rs`: 新增 `ListenerSet = 21`
- [ ] `src/types/resource/defs.rs`: 注册
- [ ] `storage.rs`: API 映射

### Phase 2: ListenerSet Handler (~2 天)
- [ ] `handlers/listener_set.rs`: 新文件
- [ ] filter/parse/on_change/on_delete/update_status 完整实现

### Phase 3: Gateway Handler 合并 (~2 天)
- [ ] parse() 合并 ListenerSet listeners
- [ ] 冲突检测
- [ ] `gateway_listenerset_index` 双向映射

### Phase 4: Route 附加 (~2 天)
- [ ] `route_utils.rs`: 支持 `kind: ListenerSet` parentRef
- [ ] `hostname_resolution.rs`: ListenerSet hostname 解析
- [ ] 各 route handler 支持

### Phase 5: Gateway 侧 (~1 天)
- [ ] `conf_sync`: 新增 `ClientCache<ListenerSet>`
- [ ] `runtime/server/base.rs`: 物理 listener 创建

### Phase 6: 跨命名空间 + ReferenceGrant (~1 天)
- [ ] ListenerSet → Gateway 跨 NS
- [ ] TLS Secret 跨 NS
- [ ] ReferenceGrant requeue

### Phase 7: 测试 (~2 天)
- [ ] 10 个 conformance test YAML
- [ ] 集成测试

## 验证

```bash
# 全部 ListenerSet 测试
./scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant,ListenerSet"

# 单个
./scripts/run.sh --run-test ListenerSetHTTPRouting
```
