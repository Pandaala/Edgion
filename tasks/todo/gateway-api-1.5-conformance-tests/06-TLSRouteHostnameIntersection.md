# TLSRouteHostnameIntersection

**Profile**: GATEWAY-TLS Core
**状态**: ❌ FAIL（`--tls-only` 模式仍失败）
**根因类别**: 多 Gateway 共享端口 443 时 SNI hostname 路由不工作

## 最新结果（2026-03-29 --tls-only 模式）

**FAIL** — 即使使用 `--tls-only` 模式（预创建了所有 4 个 hostname Gateway），此测试仍然失败。

17/18 TLS Core 通过，唯一失败就是此测试。

## 失败子测试（共 8 个子测试失败，4 个通过）

**PASS 的子测试**（不匹配 hostname 的请求被正确拒绝）:
- `TLSRoutes_intersect_with_exact_listener_hostname/TLS_request_not_matching_hostnames_intersection_should_not_reach_backend`
- `TLSRoutes_intersect_with_more_specific_wildcard_listener_hostname/TLS_request_not_matching_hostnames_intersection_should_not_reach_backend`
- `TLSRoutes_intersect_with_less_specific_wildcard_listener_hostname/TLS_request_not_matching_hostnames_intersection_should_not_reach_backend`
- `TLSRoutes_intersect_with_empty_listener_hostname/TLS_request_not_matching_hostnames_intersection_should_not_reach_backend`

**FAIL 的子测试**（匹配 hostname 的请求全部 EOF 超时 30s）:
- `exact_listener/TLS_request_matching_exact_hostnames_intersection_should_reach_backend` (abc.example.com)
- `more_specific_wildcard/TLS_request_matching_exact_hostnames_intersection_should_reach_backend` (abc.example.com)
- `more_specific_wildcard/TLS_request_matching_wildcard_hostnames_intersection_should_reach_backend_2` (other.example.com)
- `less_specific_wildcard/TLS_request_matching_exact_hostnames_intersection_should_reach_backend` (abc.example.com)
- `less_specific_wildcard/TLS_request_matching_wildcard_hostnames_intersection_should_reach_backend_2` (other.example.com)
- `empty_listener/TLS_request_matching_exact_hostnames_intersection_should_reach_backend` (abc.example.com)
- `empty_listener/TLS_request_matching_wildcard_hostnames_intersection_should_reach_backend_2` (other.example.com)
- *(以上 8 个均为匹配请求应到达 backend 但收到 EOF)*

## 错误现象

```
roundtripper.go:266: Error sending request: Get "https://192.168.194.193/": EOF (no response)
tls.go:78: Request failed, not ready yet: Get "https://192.168.194.193/": EOF (after 29.158044292s)
http.go:266: timeout while waiting after 30 attempts, 0/3 successes
```

请求发送到 `192.168.194.193:443`，TLS SNI 设置为对应 hostname（如 `abc.example.com`），
但 gateway 直接关闭了连接（EOF）。

## 根因分析

### 测试结构

4 个 Gateway 全部监听 443 端口，但 hostname 不同：
- `gw-tlsroute-exact-hostname-x-1`: hostname = `abc.example.com`
- `gw-tlsroute-more-specific-wc-hostname-x-2`: hostname = `*.example.com`
- `gw-tlsroute-less-specific-wc-hostname-x-3`: hostname = `*.com`
- `gw-tlsroute-empty-hostname-x-4`: hostname = (empty, matches all)

每个 Gateway 绑定了 TLSRoute（hostname 可能更宽或更窄），测试验证 TLSRoute 与 listener
hostname 的交集行为。

### 核心问题

测试在运行时 **Update** 这些预创建的 Gateway（`apply.go:294: Updating gw-tlsroute-*`），
添加 TLSRoute。但 **匹配请求发到 `192.168.194.193:443` 后收到 EOF**。

可能原因：
1. **多 Gateway 共享同一端口时 SNI 路由不正确**: 4 个 Gateway 都用 443 端口但 hostname
   不同，数据面需要基于 ClientHello SNI 将连接路由到正确的 Gateway listener。如果
   数据面只认第一个 Gateway 的 listener、或者 hostname 匹配逻辑有 bug，就会 EOF。
2. **Update 后路由配置未生效**: Gateway 被 update 后（添加 TLSRoute parentRef），
   controller 可能没有正确推送新的路由表给数据面。
3. **TLSRoute 的 hostname intersection 逻辑 bug**: Controller 在计算 listener hostname
   和 TLSRoute hostname 交集时，可能错误地判断为无交集，导致不绑定路由。

### 验证依据

- `TLSRouteSimpleSameNamespace` 在同一 `--tls-only` 环境下 **PASS**（单 Gateway、
  单 TLSRoute、443 端口），说明单 Gateway 的 TLS passthrough 功能正常。
- 不匹配 hostname 的请求正确返回 EOF（reject），说明 TCP accept 和基本的 SNI
  过滤是工作的。
- 匹配的请求也 EOF，说明问题出在**多 Gateway hostname 场景下路由到 backend 的逻辑**。

## 修复方向

1. **检查 controller 的 TLSRoute→listener hostname intersection 逻辑**:
   确认 controller 是否正确计算了 route hostname 和 listener hostname 的交集，
   并生成了正确的 SNI 路由规则。

2. **检查数据面 TLS passthrough 的多 hostname 路由**:
   确认当多个 Gateway/listener 共享 443 端口时，数据面是否能基于 SNI 正确
   分发到不同的 backend。

3. **检查 Update 后的配置推送**:
   确认 controller 在 Gateway 被 update（添加 TLSRoute）后是否正确重新
   计算并推送配置。

## 关联

- `05-TLSRouteSimpleSameNamespace` 已在 `--tls-only` 模式通过（单 Gateway 场景 OK）
- 此测试是 GATEWAY-TLS profile 在 `--tls-only` 模式下**唯一剩余的失败项**

## 复现命令

```bash
cd examples/gateway-api-conformance
# 先用 --tls-only 初始化
bash scripts/setup-orbstack.sh --tls-only --skip-build
# 跑测试
GO_RUN_FILTER="TestConformance/TLSRouteHostnameIntersection$" bash scripts/run.sh --conformance-profiles GATEWAY-TLS
```
