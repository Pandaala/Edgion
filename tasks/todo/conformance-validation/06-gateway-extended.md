# 阶段 6: Gateway Extended Conformance

**测试数**: 10
**预计耗时**: 1-2 天
**Features**: Gateway Extended features

> 部分测试依赖 missing-features 子任务完成后才能通过。

## 运行命令

```bash
# Gateway Extended (可独立跑的)
./scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant,GatewayHTTPListenerIsolation,GatewayPort8080"

# 单个测试
./scripts/run.sh --run-test GatewayHTTPListenerIsolation
```

## Gateway Extended (10 tests)

| # | 测试名 | 状态 | 依赖 | 备注 |
|---|--------|------|------|------|
| 1 | GatewayStaticAddresses | [ ] | P7 | |
| 2 | GatewayOptionalAddressValue | [ ] | P7 | |
| 3 | GatewayInfrastructure | [ ] | P5 | 需要实现 infrastructure 字段 |
| 4 | GatewayHTTPListenerIsolation | [ ] | 无 | 应该能直接通过 |
| 5 | GatewayInvalidTLSBackendConfiguration | [ ] | P6 | 需要实现 backend TLS |
| 6 | GatewayBackendClientCertificateFeature | [ ] | P6 | 需要实现 backend TLS |
| 7 | GatewayFrontendClientCertificateValidation | [ ] | 无 | 已实现 frontendValidation |
| 8 | GatewayFrontendClientCertificateValidationInsecureFallback | [ ] | 无 | 已实现 |
| 9 | GatewayFrontendInvalidDefaultClientCertificateValidation | [ ] | 无 | 已实现 |
| 10 | GatewayInvalidFrontendClientCertificateValidation | [ ] | 无 | 已实现 |

## 可立即验证 (无依赖)

- GatewayHTTPListenerIsolation
- GatewayFrontendClientCertificateValidation (4 个)

## 需要等缺失功能完成

- GatewayStaticAddresses / OptionalAddressValue → P7
- GatewayInfrastructure → P5
- GatewayInvalidTLSBackendConfiguration / BackendClientCertificateFeature → P6
