---
name: upstream-address-safety
description: 上游地址安全边界判定规则。审查 DynamicExternalUpstream、RegionRoute、DynamicInternalUpstream、DirectEndpoint、RequestMirror、EndpointSlice/LB 等会改写或选择 upstream 的路径时先读此文件。
---

# Upstream Address Safety

## 结论

凡是 Gateway 数据面会据此建立 upstream 连接的路径，都要同时满足两层约束：

1. **授权约束**：请求方只能选择 operator / route 已授权的目标，例如 `domainMap`、`backendRefs`、RegionRoute PM、RequestMirror backendRef。
2. **地址安全约束**：最终拨号地址必须按来源类型执行安全校验：
   - 外部/DNS 型 upstream（例如 `ExternalJumpPreset`）默认拒绝 loopback 与 link-local / metadata 地址，即 `127.0.0.0/8`、`::1`、`169.254.0.0/16`、`fe80::/10`。
   - K8s Endpoint / EndpointSlice 派生 upstream 默认同样拒绝 loopback 与 link-local / metadata；只有显式 integration testing mode 可为了本地 fixture 放行 Endpoint 派生的 loopback。
   - `EdgionGatewayConfig.spec.securityProtect.allowLoopbackUpstream=true` 是 GatewayClass 级 operator override，只放行 loopback，不放行 link-local / metadata。

RFC1918 私网地址（`10/8`、`172.16/12`、`192.168/16`）默认允许；K8s Service、Pod、跨集群网关、企业内网 API 都依赖这些地址。

## Review 判定

如果某条路径只做了授权约束，但在最终拨号前没有执行地址安全约束，算真实问题，通常定为 **P2 安全加固 / 防线绕过**。它不是外部用户任意 SSRF，但会绕过 Edgion 对云 metadata / link-local 的既有防线。外部/DNS 型 upstream 漏掉 loopback 也按同类问题处理。

接受的修复方向：

- 复用 `classify_backend_addr()` / `validate_backend_addr()` 或等价逻辑。
- DNS 域名必须在解析成地址后校验，不能只校验字符串。
- 校验点尽量靠近最终 `HttpPeer` / `reqwest` / TCP dial 创建处，避免插件间绕过。

不接受的修复方向：

- 阻断所有私网 CIDR。
- 只在配置校验阶段做 DNS 解析。
- 只靠 `domainMap` / `backendRefs` 这类 allowlist 证明安全；allowlist 是授权边界，不等于 metadata 地址防线。

## 已盘点路径

| 路径 | 当前语义 | Review 要点 |
|---|---|---|
| `DynamicExternalUpstream` | 请求值只能命中 `domainMap`，但目标域名/IP 可由配置给出 | 需要解析后拒绝 loopback + link-local；只拒绝 loopback 不够 |
| `RegionRoute` | PM 中 `RegionDef.backendEndpoint` 通过 `ExternalJumpPreset` 走外部域名拨号 | 和 DynamicExternalUpstream 同类，也要解析后校验 |
| `DynamicInternalUpstream` | 只能选择当前 route `backendRefs` 中已有 BackendRef | 授权约束较好；最终地址安全应由被选 BackendRef 的主后端拨号路径统一保证 |
| `DirectEndpoint` | 请求给 IP，但必须存在于当前 route `backendRefs` 对应 Endpoint/EndpointSlice | endpoint membership 只证明“属于已授权 Service”，不自动证明地址安全；最终直连地址也应校验 |
| `RequestMirror` | 镜像 backendRef 由配置指定，独立于主请求成功/失败 | LB 选中 endpoint 后应和主后端一致校验；不允许缺少 endpoint store 时回退到 unchecked K8s DNS + reqwest 自行解析 |
| `AllEndpointStatus` | 请求触发，对当前 route 后端所有 endpoint fan-out | 虽不是主 upstream，但属于数据面 endpoint 拨号，fan-out 前必须过滤 unsafe endpoint 并写 bounded plugin log |
| Endpoint / EndpointSlice LB | operator/K8s 提供 endpoint，主请求会直接拨号 | LB 选中地址必须靠近 `HttpPeer` 创建处校验；生产默认拒绝 loopback + link-local |

## 当前兼容性约束

Edgion 的本地 integration fixtures 依赖 `127.0.0.1` Endpoint/EndpointSlice，因此测试/开发兼容性必须显式隔离，不能影响生产默认策略。当前推荐分层：

- 外部/DNS jump：默认拒绝 loopback + link-local；operator override 只可放行 loopback。
- Endpoint/EndpointSlice 派生拨号：生产默认拒绝 loopback + link-local；`--integration-testing-mode` 可放行 loopback 以支持本地 fixture，但仍拒绝 link-local / metadata；operator override 同样只可放行 loopback。

## 与 ForwardAuth / Webhook 规则的关系

[forward-auth-webhook-ssrf.md](forward-auth-webhook-ssrf.md) 关闭的是“静态 operator URI 应禁止内网访问”类 finding，因为 ForwardAuth/Webhook 的合法用途就是访问内网服务，且一刀切私网阻断会破坏功能。

本规则更窄：不禁止内网，只禁止 loopback 与 link-local / metadata；并且关注的是 Gateway upstream 拨号路径之间的安全策略一致性。
