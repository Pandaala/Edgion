# P4: Gateway Frontend Client Certificate Validation

**工作量**: ~5 天
**测试**: GatewayFrontendClientCertificateValidation, ...InsecureFallback, ...InvalidDefault, ...InvalidFrontend
**技术方案**: `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md` § P4

## 问题

Gateway listener 需要支持 `tls.frontendValidation` 字段实现入口 mTLS。

## 实现步骤

### Controller 侧
- [ ] `src/types/resources/gateway.rs`: 新增 `FrontendTLSValidation` struct
- [ ] `handlers/gateway.rs` parse(): 解析 `ca_certificate_refs`，从 ConfigMap/Secret 获取 CA PEM
- [ ] `handlers/gateway.rs` validate: 新增 `InvalidCACertRef` 等错误变体
- [ ] 新增 `ConfigMapRefManager`（仿 SecretRefManager）

### Gateway 侧
- [ ] `runtime/matching/tls.rs`: `GatewayTlsEntry` 新增 frontend CA 字段
- [ ] `tls/runtime/gateway/tls_pingora.rs`: `configure_gateway_frontend_mtls()`
- [ ] 复用 EdgionTls 的 `configure_mtls()` 逻辑

### 测试
- [ ] 集成测试：正常 mTLS
- [ ] 集成测试：optional 模式（允许无客户端证书）
- [ ] 集成测试：无效 CA ref

## 验证

```bash
./scripts/run.sh --run-test GatewayFrontendClientCertificateValidation
./scripts/run.sh --run-test GatewayFrontendClientCertificateValidationInsecureFallback
./scripts/run.sh --run-test GatewayFrontendInvalidDefaultClientCertificateValidation
./scripts/run.sh --run-test GatewayInvalidFrontendClientCertificateValidation
```
