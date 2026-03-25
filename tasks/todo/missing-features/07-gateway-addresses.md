# P7: Gateway Static/Optional Addresses

**工作量**: ~0.5 天
**测试**: GatewayStaticAddresses, GatewayOptionalAddressValue
**技术方案**: `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md` § P7

## 现有实现

`derive_gateway_addresses()` 已处理 spec.addresses → status.addresses。

## 实现步骤

- [ ] 确认 status condition 正确反映地址分配结果
- [ ] 在 conformance_test.go 中声明 `GatewayStaticAddresses` 和 `GatewayAddressEmpty`
- [ ] 跑测试验证

## 验证

```bash
./scripts/run.sh --run-test GatewayStaticAddresses
./scripts/run.sh --run-test GatewayOptionalAddressValue
```
