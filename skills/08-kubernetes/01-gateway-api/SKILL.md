---
name: gateway-api
description: Compatibility notes for Edgion's Gateway API handling, including intentional deviations and not-implemented features.
---

# Gateway API Compatibility Notes

Use this note when changing Gateway API behavior in Edgion and you need to understand intentional compatibility boundaries.

## 文件索引

| 文件 | 内容 |
|------|------|
| [01-not-implemented.md](01-not-implemented.md) | 不实现/暂不实现的 Gateway API 功能：addresses 上报、跨类型 Hostname 冲突、infrastructure、`ListenerSet`、`HTTPRouteRequestMultipleMirrors`、BTP、`TLSRouteModeMixed`、TLS profile；已完成项：~~frontendValidation~~ ✅ |
| [02-conformance-testing.md](02-conformance-testing.md) | 如何运行、调试和逐步验证 Gateway API conformance tests：脚本用法、粒度控制、Port-Forward、调试技巧 |
| [03-conformance-workflow.md](03-conformance-workflow.md) | 跨会话工作流：逐项推进 conformance 测试（环境检查→功能分析→运行→诊断→记录） |
| [04-api-version-upgrade.md](04-api-version-upgrade.md) | K8s API 生命周期与 Controller 升级策略 |
| [05-batch-conformance-debugging.md](05-batch-conformance-debugging.md) | 批量 conformance 失败的日志采集、时序关联和根因定位（Batch 7 专用） |


## ConformanceProfiles 声明状态

Edgion 在 conformance 测试中声明以下 profiles（GEP-1709）：

| Profile | 状态 | 说明 |
|---------|------|------|
| `GATEWAY-HTTP` | ✅ 已声明 | Core: Gateway + HTTPRoute + ReferenceGrant |
| `GATEWAY-GRPC` | ⚠️ 已声明，未验证 | Core: Gateway + GRPCRoute + ReferenceGrant。功能已实现，但未实际跑过 conformance test，可能需要修 status condition |
| `GATEWAY-TLS` | ❌ 未声明 | TLS Passthrough 已实现；TLS Terminate 走标准 `mode=Terminate` 路径但仍需补 status 与测试。Mixed 明确不支持；需跑通 TLSRoute conformance 后再声明 |
| `MESH-HTTP` | ❌ 不适用 | Edgion 不是 service mesh |
| `MESH-GRPC` | ❌ 不适用 | Edgion 不是 service mesh |

配置位置：`examples/gateway-api-conformance/conformance_test.go`（`opts.ConformanceProfiles`）

## TLS Certificate Selection

Edgion intentionally does not support cross-port TLS certificate fallback, but does
support catch-all (no hostname) listener certificate selection as a per-port fallback.

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

## Why catch-all TLS matching is limited to per-port scope

Gateway API allows a Listener without `hostname`, meaning the listener matches all hostnames
at the routing level. Edgion honors this by using the catch-all listener's certificate as a
per-port fallback when no hostname-specific listener matches the SNI. The certificate may not
be valid for every SNI, but this matches Gateway API conformance expectations — the catch-all
listener is the correct fallback within its port.

## Why cross-port fallback is not supported

Gateway listeners are port-scoped. Allowing a fallback that ignores port breaks listener isolation and can select the wrong certificate when the same hostname is configured differently on multiple ports.

Edgion treats `(port, sni)` as the required lookup key for Gateway TLS.

## EdgionTls binding rule

For `EdgionTls`, controller-side port resolution is required before the resource can enter the matcher.

- If parent refs resolve to listener ports, the TLS resource is admitted into the matcher.
- If no listener port is resolved, the TLS resource is skipped from the matcher.
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
and EdgionTls to silently get `resolved_ports = None` when parentRef had only
`name` + `namespace`.

**Test coverage:** `TLSRoute/BothAbsentParentRef` and
`EdgionTls/BothAbsentParentRef` integration tests verify this behavior.

## EdgionTls Requeue via gateway_route_index

`EdgionTls` must register in `gateway_route_index` via its `on_change()` method.
Without this, when a Gateway is added or its listeners change, EdgionTls would
not be requeued to re-resolve its listener ports.

**Bug history (2026-03):** EdgionTls originally had no `on_change()` at all.
If EdgionTls was processed before its Gateway during init, `resolved_ports`
would permanently be `None` with no requeue mechanism. Fixed by adding
`on_change()` with `update_gateway_route_index()` and `on_delete()` with
`remove_from_gateway_route_index()`.

**Rule:** Any handler that calls `lookup_gateway()` in `parse()` MUST register
in `gateway_route_index` via `on_change()` so that Gateway changes trigger
requeue. Currently this applies to: TLSRoute, EdgionTls, HTTPRoute, GRPCRoute,
TCPRoute, UDPRoute.

## Implementation guidance

When editing TLS matching:

- keep `EdgionTls` matching port-scoped
- keep Gateway TLS matching port-scoped
- catch-all (no hostname) listeners are valid per-port fallbacks — do not remove this support
- do not reintroduce cross-port fallback lookup
- prefer explicit status/reporting over implicit matcher fallback
