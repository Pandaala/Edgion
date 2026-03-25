# P6: Gateway Backend TLS Configuration

**工作量**: ~3 天
**测试**: GatewayInvalidTLSBackendConfiguration, GatewayBackendClientCertificateFeature
**技术方案**: `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md` § P6

## 实现步骤

- [ ] `src/types/resources/gateway.rs`: 新增 `GatewaySpecTLS`, `GatewayBackendTLS`
- [ ] `handlers/gateway.rs` parse(): 解析 client cert Secret ref
- [ ] `handlers/gateway.rs` validate: 无效 ref → `ResolvedRefs=False`
- [ ] `pg_upstream_peer.rs`: 无 BackendTLSPolicy 时 fallback 到 Gateway 级 client cert
- [ ] `listener_builder.rs`: 将 Gateway backend TLS config 传入
- [ ] 集成测试

## 优先级

Gateway client cert 是默认值，BackendTLSPolicy 可逐 backend 覆盖。

## 验证

```bash
./scripts/run.sh --run-test GatewayInvalidTLSBackendConfiguration
./scripts/run.sh --run-test GatewayBackendClientCertificateFeature
```
