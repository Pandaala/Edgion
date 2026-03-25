# P3: TLSRoute Mixed Termination

**工作量**: ~5 天
**依赖**: P2 (TLSRoute Terminate)
**测试**: TLSRouteMixedTerminationSameNamespace
**技术方案**: `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md` § P3

## 架构

同端口单 TCP listener，应用层根据 SNI 决定透传或终止。

## 实现步骤

- [ ] `src/types/resources/tls_route.rs`: 新增 `TlsTerminationMode` enum
- [ ] `src/core/gateway/routes/tls/gateway_tls_routes.rs`: route table 携带 mode
- [ ] `handlers/gateway.rs`: 解析 parentRef 时传播 listener tls.mode
- [ ] `src/core/gateway/routes/tls/mixed_proxy.rs`: **新文件** `EdgionTlsMixedProxy`
- [ ] `src/core/gateway/routes/tls/common.rs`: **新文件** 共享逻辑
- [ ] `listener_builder.rs`: 同端口不同 mode 时自动启用 Mixed proxy
- [ ] 集成测试：同端口 Passthrough + Terminate

## 最难点

应用层 TLS 握手：在已读取 ClientHello 的 raw TCP stream 上完成 TLS handshake。

## 验证

```bash
./scripts/run.sh --run-test TLSRouteMixedTerminationSameNamespace
```
