# Missing Features — 缺失功能实现

目标：实现 Gateway API v1.5 conformance tests 要求但 Edgion 尚未实现的 8 个功能。

## 总览

| # | 功能 | 文件 | 测试数 | 工作量 | 依赖 | 状态 |
|---|------|------|--------|--------|------|------|
| P1 | Multiple Mirrors | [01-multiple-mirrors.md](01-multiple-mirrors.md) | 1 | ~1 天 | 无 | [ ] |
| P2 | TLSRoute Terminate | [02-tlsroute-terminate.md](02-tlsroute-terminate.md) | 3 | ~3 天 | 无 | [ ] |
| P3 | TLSRoute Mixed | [03-tlsroute-mixed.md](03-tlsroute-mixed.md) | 1 | ~5 天 | P2 | [ ] |
| P4 | Frontend Client Cert | [04-frontend-client-cert.md](04-frontend-client-cert.md) | 4 | ~5 天 | 无 | [ ] |
| P5 | Gateway Infrastructure | [05-gateway-infrastructure.md](05-gateway-infrastructure.md) | 1 | ~2 天 | 无 | [ ] |
| P6 | Gateway Backend TLS | [06-gateway-backend-tls.md](06-gateway-backend-tls.md) | 2 | ~3 天 | 无 | [ ] |
| P7 | Gateway Addresses | [07-gateway-addresses.md](07-gateway-addresses.md) | 2 | ~0.5 天 | 无 | [ ] |
| P8 | ListenerSet | [08-listener-set.md](08-listener-set.md) | 10 | ~11 天 | 无 | [ ] |

**总计**: 24 tests, ~30.5 天

## 建议执行顺序

```
Day 1:     P1 (Multiple Mirrors) + P7 (Gateway Addresses)     — 两个小任务一天搞定
Day 2-4:   P2 (TLSRoute Terminate)                            — 中等难度
Day 5-9:   P4 (Frontend Client Cert)                          — 大任务
Day 10-12: P6 (Gateway Backend TLS)                           — 中等难度
Day 13-17: P3 (TLSRoute Mixed)                                — 大任务，依赖 P2
Day 18-19: P5 (Gateway Infrastructure)                        — 中等难度
Day 20-30: P8 (ListenerSet)                                   — 最大工作量
```

## 技术方案参考

详细设计见 `tasks/todo/gateway-api-1.5-conformance-tests/20-implementation-plans.md`
