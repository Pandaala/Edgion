# P1: HTTPRoute Multiple Mirrors

**工作量**: ~1 天
**测试**: HTTPRouteRequestMultipleMirrors
**技术方案**: `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md` § P1

## 问题

当前单 rule 只支持一个 mirror，`ctx.mirror_state: Option<MirrorState>` 后设覆盖前设。

## 实现步骤

- [ ] `src/types/ctx.rs`: `mirror_state: Option<MirrorState>` → `mirror_states: Vec<MirrorState>`
- [ ] `src/core/gateway/plugins/runtime/traits/session.rs`: `set_mirror_state()` → `add_mirror_state()`
- [ ] `src/core/gateway/plugins/runtime/session_adapter.rs`: push 语义
- [ ] `src/core/gateway/plugins/http/request_mirror/plugin.rs`: 调用改为 push
- [ ] `src/core/gateway/routes/http/proxy_http/pg_request_body_filter.rs`: body tee 遍历 Vec
- [ ] `src/core/gateway/routes/http/proxy_http/pg_logging.rs`: drain 语义
- [ ] 集成测试：双 mirror HTTPRoute，验证两个 mirror backend 都收到请求

## 验证

```bash
./scripts/run.sh --run-test HTTPRouteRequestMultipleMirrors
```
