# P5: Gateway Infrastructure

**工作量**: ~2 天
**测试**: GatewayInfrastructure
**技术方案**: `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md` § P5

## 实现步骤

- [ ] `src/types/resources/gateway.rs`: 新增 `GatewayInfrastructure` struct
- [ ] `handlers/gateway.rs`: parse() 解析 infrastructure 字段
- [ ] on_change() 可选 patch Service 标签
- [ ] 集成测试

## 验证

```bash
./scripts/run.sh --run-test GatewayInfrastructure
```
