# TLSRouteInvalidBackendRefNonexistent

**Profile**: GATEWAY-TLS Core
**状态**: ✅ PASS（`--tls-only` 模式）/ ❌ FAIL（全量模式）
**根因类别**: TLS passthrough 与 HTTPS terminate 端口冲突（全量模式特有）

## 最新结果（2026-03-29 --tls-only 模式）

**PASS** — 使用 `setup-orbstack.sh --tls-only` 后此测试通过。

`--tls-only` 模式下 443 端口为 TLS Passthrough，测试动态创建的 Gateway 能正确生效，
backend 不存在时连接被正确拒绝。

## 结论

此测试不存在功能 bug，是全量模式下端口冲突导致的环境问题。`--tls-only` 模式已通过。

## 复现命令

```bash
cd examples/gateway-api-conformance
bash scripts/setup-orbstack.sh --tls-only --skip-build
GO_RUN_FILTER="TestConformance/TLSRouteInvalidBackendRefNonexistent$" bash scripts/run.sh --conformance-profiles GATEWAY-TLS
```
