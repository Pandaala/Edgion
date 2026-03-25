---
name: gateway-api
description: Compatibility notes for Edgion's Gateway API handling, including intentional deviations and not-implemented features.
---

# Gateway API Compatibility Notes

Use this note when changing Gateway API behavior in Edgion and you need to understand intentional compatibility boundaries.

## 文件索引

| 文件 | 内容 |
|------|------|
| [01-not-implemented.md](01-not-implemented.md) | 不实现/暂不实现的 Gateway API 功能：addresses 上报、跨类型 Hostname 冲突、infrastructure、BTP、TLS profile；已完成项：~~frontendValidation~~ ✅ |
| [02-conformance-testing.md](02-conformance-testing.md) | 如何运行、调试和逐步验证 Gateway API conformance tests：脚本用法、粒度控制、Port-Forward、调试技巧 |

## ConformanceProfiles 声明状态

Edgion 在 conformance 测试中声明以下 profiles（GEP-1709）：

| Profile | 状态 | 说明 |
|---------|------|------|
| `GATEWAY-HTTP` | ✅ 已声明 | Core: Gateway + HTTPRoute + ReferenceGrant |
| `GATEWAY-GRPC` | ⚠️ 已声明，未验证 | Core: Gateway + GRPCRoute + ReferenceGrant。功能已实现，但未实际跑过 conformance test，可能需要修 status condition |
| `GATEWAY-TLS` | ❌ 未声明 | TLS Passthrough 已实现，但 conformance tests 从未跑过。需加 `SupportTLSRoute` 并修 status conditions（ResolvedRefs、supportedKinds）后再声明 |
| `MESH-HTTP` | ❌ 不适用 | Edgion 不是 service mesh |
| `MESH-GRPC` | ❌ 不适用 | Edgion 不是 service mesh |

配置位置：`examples/gateway-api-conformance/conformance_test.go`（`opts.ConformanceProfiles`）

## TLS Certificate Selection

Edgion intentionally does not support two looser Gateway API-adjacent TLS behaviors:

1. Listener catch-all for TLS certificate selection
2. Cross-port TLS certificate fallback

### What Edgion does

- TLS certificate selection must match the current listener port.
- Listener `hostname` must be present to participate in Gateway TLS certificate matching.
- Listener entries without `hostname` are ignored for dynamic TLS certificate selection.
- If no `(port, sni)` match exists, certificate selection fails.
- Explicit `fallback_sni` remains an Edgion-specific override, not a generic catch-all.

### What Edgion does not do

- No hostname-less catch-all listener certificate selection
- No search across all listener ports for a usable certificate
- No implicit fallback from one listener's certificate to another listener on a different port

## Why catch-all TLS matching is not supported

Gateway API allows a Listener without `hostname`, but that only means the listener matches all hostnames at the routing level. It does not guarantee the configured certificate is valid for every incoming SNI.

For TLS termination, Edgion prefers strict certificate selection rules:

- exact SNI match
- wildcard SNI match
- explicit fallback configuration

This avoids selecting a certificate that has no clear relation to the requested SNI.

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
- do not reintroduce hostname-less catch-all certificate selection
- do not reintroduce cross-port fallback lookup
- prefer explicit status/reporting over implicit matcher fallback
