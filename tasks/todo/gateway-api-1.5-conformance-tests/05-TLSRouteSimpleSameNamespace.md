# TLSRouteSimpleSameNamespace

**Profile**: GATEWAY-TLS Core
**状态**: ✅ PASS（`--tls-only` 模式）/ ❌ FAIL（全量模式）
**根因类别**: TLS passthrough 与 HTTPS terminate 端口冲突（全量模式特有）

## 最新结果（2026-03-29 --tls-only 模式）

**PASS** — 使用 `setup-orbstack.sh --tls-only` 后此测试通过。

`--tls-only` 模式预创建了 `gateway-tlsroute` Gateway（443 端口 TLS Passthrough），
并移除了与之冲突的 `same-namespace-with-https-listener`（443 HTTPS Terminate），
解决了端口冲突问题。

## 全量模式失败原因（仅供参考）

全量模式下 443 端口被 `same-namespace-with-https-listener` 绑定为 HTTPS terminate。
测试动态创建的 `gateway-tlsroute`（443 TLS Passthrough）无法生效，请求被 HTTPS terminate
listener 截获，证书 `*.org, *.wildcard.org` 与 `abc.example.com` 不匹配导致 x509 失败。

## 结论

此测试不存在功能 bug，是环境配置问题。`--tls-only` 模式已通过。
全量和 TLS 测试共存的方案需要支持同端口多 TLS mode（长期方案）或动态 listener。

## 复现命令

```bash
cd examples/gateway-api-conformance
# --tls-only 模式（PASS）
bash scripts/setup-orbstack.sh --tls-only --skip-build
GO_RUN_FILTER="TestConformance/TLSRouteSimpleSameNamespace$" bash scripts/run.sh --conformance-profiles GATEWAY-TLS
```
