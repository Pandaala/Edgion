# HTTPRouteHTTPSListenerDetectMisdirectedRequests

**Profile**: GATEWAY-HTTP Extended
**状态**: FAIL
**根因类别**: 数据面未检测 SNI/Host header mismatch

## 失败子测试

`1_request_to_'second-example.org/detect-misdirected-requests'_should_receive_one_of_[421]`

子测试 0（正常请求 example.org → infra-backend-v1）通过。

## 错误现象

```
tls.go:83: Response expectation failed for request:
  {URL: https://192.168.194.193/detect-misdirected-requests,
   Host: second-example.org,
   Protocol: H2,
   ServerName: example.org}
  not ready yet: expected status code to be one of [421], got 200.

tls.go:84: Full response:
  {Path:/detect-misdirected-requests Host:second-example.org Method:GET Protocol:HTTP/1.1
   Headers:map[...X-Forwarded-For:[192.168.194.0] X-Real-Ip:[192.168.194.0]]
   HTTPPort:3000 Namespace:gateway-conformance-infra Pod:infra-backend-v2-...}
```

## 根因分析

测试场景：
1. TLS 连接使用 `ServerName: example.org`（TLS SNI）
2. HTTP/2 请求发送 `Host: second-example.org`（HTTP Host header）
3. SNI 和 Host header **不匹配**

**期望行为**: Gateway 检测到 SNI 和 Host header 不匹配，返回 HTTP 421 Misdirected Request

**实际行为**: Gateway 正常路由请求到 `second-example.org` 的后端（infra-backend-v2），返回 200。Gateway 完全忽略了 SNI/Host 不一致。

### 技术细节

按 Gateway API 规范（GEP-TBD），当 HTTPS listener 检测到以下情况时应返回 421：
- TLS SNI（ClientHello 中的 server_name）与 HTTP Host header 不匹配
- 这防止了"confused deputy"攻击，客户端可能被重定向到错误的后端

Edgion 当前行为：
- TLS 握手后，根据 HTTP Host header 做路由（忽略 SNI）
- 没有 SNI vs Host 的一致性检查

## 修复方向

- **文件**: gateway 数据面的 HTTPS 请求处理逻辑
- **修改点**: 在 HTTPS listener 收到 HTTP 请求时，比较 TLS SNI 和 HTTP Host header
  1. 从 TLS 连接中提取 SNI（`ClientHello.server_name`）
  2. 从 HTTP 请求中提取 Host header
  3. 如果 SNI 存在且与 Host 不匹配，返回 HTTP 421 Misdirected Request
- **注意**: 只在 HTTPS listener 上做此检查，HTTP listener 不需要
- **feature 声明**: `conformance_test.go` 中 `features.SupportGatewayHTTPSListenerDetectMisdirectedRequests`

### 实现参考

```
if tls_sni.is_some() && tls_sni != http_host {
    return Response::builder()
        .status(421)
        .body("Misdirected Request")
}
```

## 复现命令

```bash
cd examples/gateway-api-conformance
GO_RUN_FILTER="TestConformance/HTTPRouteHTTPSListenerDetectMisdirectedRequests$" bash scripts/run.sh
```
