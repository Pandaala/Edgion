---
name: gateway-api
description: Compatibility notes for Edgion's Gateway API handling, including intentional deviations and not-implemented features.
---

# Gateway API Compatibility Notes

Use this note when changing Gateway API behavior in Edgion and you need to understand intentional compatibility boundaries.

## 文件索引

| 文件 | 内容 |
|------|------|
| [01-not-implemented.md](01-not-implemented.md) | 不实现/暂不实现的 Gateway API 功能：addresses 上报、跨类型 Hostname 冲突、infrastructure、`ListenerSet`、`HTTPRouteRequestMultipleMirrors`、BackendTLSPolicy conformance（已实现，SANValidation 暂不声明）、BackendTrafficPolicy、`TLSRouteModeMixed`、TLS profile；已完成项：~~frontendValidation~~ ✅ |
| [02-conformance-testing.md](02-conformance-testing.md) | 如何运行、调试和逐步验证 Gateway API conformance tests：脚本用法、粒度控制、Port-Forward、调试技巧 |
| [03-conformance-workflow.md](03-conformance-workflow.md) | 跨会话工作流：逐项推进 conformance 测试（环境检查→功能分析→运行→诊断→记录） |
| [05-batch-conformance-debugging.md](05-batch-conformance-debugging.md) | 批量 conformance 失败的日志采集、时序关联和根因定位（Batch 7 专用） |
| [06-conformance-debug-cookbook.md](06-conformance-debug-cookbook.md) | Conformance 排障手册：Pre-flight 检查、常见失败模式（namespace 被删、端口未绑定、进程残留等）、高效运行技巧 |


## Conformance 最新结果（feature-03-29, 2026-04-01）

**381 PASS / 7 FAIL / 56 SKIP**（in-cluster 模式，OrbStack K8s）

### Profile 声明状态

| Profile | 声明 | 结果 |
|---------|------|------|
| `GATEWAY-HTTP` | ✅ | Core 全部通过；2 个 HTTPS listener 数据面 FAIL |
| `GATEWAY-GRPC` | ✅ | Core 全部通过（含 weight/header/hostname matching） |
| `GATEWAY-TLS` | ✅ | Status tests 大部分通过；5 个 FAIL（数据面 + 2 个 status reason） |
| `MESH-*` | ❌ | 不适用（Edgion 不是 service mesh），56 个 SKIP 中 21 个是 Mesh |

### 7 个 FAIL

| 测试 | 类别 | 原因 |
|------|------|------|
| `HTTPRouteHTTPSListener` | HTTPS 数据面 | HTTPS 请求 15s 超时 |
| `HTTPRouteHTTPSListenerDetectMisdirectedRequests` | HTTPS 数据面 | 421 检测超时 |
| `TLSRouteSimpleSameNamespace` | TLS Passthrough 数据面 | client EOF |
| `TLSRouteTerminateSimpleSameNamespace` | TLS Terminate 数据面 | 15s 超时 |
| `TLSRouteHostnameIntersection` | TLS 数据面 | hostname 交集匹配不生效 |
| `TLSRouteListenerMixedTerminationNotSupported` | Controller Status | 返回 `UnsupportedValue`，预期 `ProtocolConflict` |
| `TLSRouteListenerTerminateSupportedKinds` | Controller Status | Terminate listener SupportedKinds 超时 |

配置位置：`examples/gateway-api-conformance/conformance_test.go`（`opts.ConformanceProfiles`）

## TLS Certificate Selection

Edgion intentionally does not support cross-port TLS certificate fallback, but does
support catch-all (no hostname) listener certificate selection as a per-listener fallback.

### What Edgion does

- TLS certificate selection must match the current listener port.
- Lookup priority per port: exact hostname > wildcard hostname > catch-all (no hostname) listener.
- A listener without `hostname` serves as the catch-all fallback for its port — its certificate is used when no hostname-specific listener matches the SNI.
- Only the first catch-all listener per port is kept; duplicates are logged as warnings.
- If no match exists (no hostname-specific and no catch-all), certificate selection fails.
- Explicit `fallback_sni` remains an Edgion-specific override for clients that send no SNI at all.

### What Edgion does not do

- No search across all listener ports for a usable certificate
- No implicit fallback from one listener's certificate to another listener on a different port

## Why catch-all TLS matching is limited to per-listener scope

Gateway API allows a Listener without `hostname`, meaning the listener matches all hostnames
at the routing level. Edgion honors this by using the catch-all listener's certificate as a
per-listener fallback when no hostname-specific listener matches the SNI. The certificate may not
be valid for every SNI, but this matches Gateway API conformance expectations — the catch-all
listener is the correct fallback within its listener scope.

## Why cross-listener fallback is not supported

Gateway listeners are listener-scoped (keyed by `ListenerKey` = `"{addr}/{protocol}"`). Allowing a fallback that ignores the listener key breaks listener isolation and can select the wrong certificate when the same hostname is configured differently on multiple listeners.

Edgion treats `(ListenerKey, sni)` as the required lookup key for Gateway TLS.

## EdgionTls binding rule

For `EdgionTls`, controller-side listener resolution is required before the resource can enter the matcher.

- If parent refs resolve to listener keys (`ListenerKey`), the TLS resource is admitted into the matcher.
- If no listener key is resolved, the TLS resource is skipped from the matcher.
- This state should be reflected in status instead of being hidden behind a global fallback.

## parentRef Resolution: both-absent Fallback

Per Gateway API spec, when a `parentRef` specifies neither `port` nor
`sectionName`, the resource MUST attach to **all listeners** of the referenced
Gateway. This applies to:

- `TLSRoute` (port resolution in `tls_route.rs`)
- `EdgionTls` (port resolution in `edgion_tls.rs`)
- `HTTPRoute` / `GRPCRoute` (hostname resolution in `hostname_resolution.rs`)

**Bug history (2026-03):** The original implementation only handled `port` set
or `sectionName` set, missing the both-absent fallback. This caused TLSRoute
and EdgionTls to silently get `resolved_listeners = None` (now `Vec<ListenerKey>`)
when parentRef had only `name` + `namespace`.

**Test coverage:** `TLSRoute/BothAbsentParentRef` and
`EdgionTls/BothAbsentParentRef` integration tests verify this behavior.

## EdgionTls Requeue via gateway_route_index

`EdgionTls` must register in `gateway_route_index` via its `on_change()` method.
Without this, when a Gateway is added or its listeners change, EdgionTls would
not be requeued to re-resolve its listener ports.

**Bug history (2026-03):** EdgionTls originally had no `on_change()` at all.
If EdgionTls was processed before its Gateway during init, `resolved_listeners`
would permanently be empty with no requeue mechanism. Fixed by adding
`on_change()` with `update_gateway_route_index()` and `on_delete()` with
`remove_from_gateway_route_index()`.

**Rule:** Any handler that calls `lookup_gateway()` in `parse()` MUST register
in `gateway_route_index` via `on_change()` so that Gateway changes trigger
requeue. Currently this applies to: TLSRoute, EdgionTls, HTTPRoute, GRPCRoute,
TCPRoute, UDPRoute.

## Implementation guidance

When editing TLS matching:

- keep `EdgionTls` matching listener-scoped (keyed by `ListenerKey`)
- keep Gateway TLS matching listener-scoped
- catch-all (no hostname) listeners are valid per-listener fallbacks — do not remove this support
- do not reintroduce cross-listener fallback lookup
- prefer explicit status/reporting over implicit matcher fallback
