# P2: TLSRoute Terminate 完善

**工作量**: ~3 天
**测试**: TLSRouteTerminateSimpleSameNamespace, TLSRouteListenerTerminateNotSupported, TLSRouteListenerMixedTerminationNotSupported
**技术方案**: `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md` § P2

## 现有实现

- `EdgionTlsTcpProxy` (proxy.rs): Terminate 模式已存在
- `EdgionTlsPassthroughProxy` (passthrough_proxy.rs): Passthrough 模式已存在

## 实现步骤

- [ ] 验证 Terminate 端到端：后端收到明文 TCP
- [ ] `handlers/gateway.rs`: `compute_supported_kinds()` 根据 TLS mode 返回正确 supportedKinds
- [ ] 不支持的 mode → `Accepted=False, reason=UnsupportedTLSMode`
- [ ] 集成测试：TLSRoute Terminate 端到端
- [ ] 集成测试：不支持的 TLS mode 返回正确 status

## 验证

```bash
./scripts/run.sh --run-test TLSRouteTerminateSimpleSameNamespace
./scripts/run.sh --run-test TLSRouteListenerTerminateNotSupported
./scripts/run.sh --run-test TLSRouteListenerMixedTerminationNotSupported
```
