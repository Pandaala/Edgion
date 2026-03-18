# test_server Capabilities

这个文件用于确认本地集成测试后端到底已经提供了什么，避免把“测试后端没实现”误判成 gateway 或 controller 问题。

## 默认监听端口

`start_all_with_conf.sh` 默认会这样启动 `test_server`：

| 能力 | 端口 |
|------|------|
| HTTP backends | `30001,30002,30003` |
| gRPC backends | `30021,30022,30023` |
| WebSocket | `30005` |
| TCP echo | `30010` |
| UDP echo | `30011` |
| TCP Proxy Protocol v2 | `30012` |
| Fake auth / OIDC | `30040` |
| HTTPS backend | `30051` |
| HTTPS backend mTLS | `30052` |

如果你单独运行 `test_server`，CLI 默认值只会起第一组端口；集成脚本会把多端口和可选 TLS/auth 端口一起传进去。

## HTTP 端点

### 基础回显与健康检查

- `/health`
- `/echo`
- `/headers`
- `/{*path}` catch-all

### Header / request-response 相关

- `/request-header-test`
- `/both-headers-test`
- `/multi-request-headers`
- `/response-header-test`
- `/auth-header-probe`

这些端点常用来验证：

- request/response header modifier
- 安全头
- forward auth 透传或清洗的头
- 上游收到的真实 header 形态

### Webhook / 动态解析

- `/webhook/resolve`
- `/webhook/resolve-body`
- `/webhook/healthz`

这些端点主要给 webhook / dynamic upstream / external resolver 相关场景复用。

### 状态码与延迟

- `/status/{code}`
- `/delay/{seconds}`

适合做：

- timeout
- retry
- 失败回退
- 特定状态码断言

### Mirror 测试端点

- `/mirror/capture`
- `/mirror/query/{trace_id}`
- `/mirror/slow/{ms}`
- `/mirror/reset`

`RequestMirror` 相关测试通常会：

1. 先发带 `x-trace-id` 的主请求
2. 通过 `/mirror/query/{trace_id}` 轮询镜像请求是否到达
3. 必要时用 `/mirror/reset` 清理现场

## Fake Auth / OIDC 端点

认证相关测试不要自己再造一个 auth mock，先看这里是否已覆盖：

- `/verify`
- `/oidc/.well-known/openid-configuration`
- `/oidc/jwks`
- `/oidc/introspect`
- `/health`

这些端点已经能覆盖：

- `ForwardAuth`
- `OpenidConnect`
- token introspection
- JWKS 拉取

## 协议级服务

- gRPC：用于 `GRPCRoute` 与 `EdgionTls/grpctls` 场景
- WebSocket：`/ws`
- TCP：普通 echo
- UDP：普通 echo
- TCP PP2：带 Proxy Protocol v2 解析能力

如果你的测试只是需要一个标准 echo、TLS 握手、OIDC/JWKS/mock auth，优先复用现有服务。

## TLS 相关后端

`test_server` 还能在提供证书后起两类 TLS 后端：

- HTTPS backend：给 backend TLS / HTTPS upstream 场景
- HTTPS backend mTLS：给上游双向 TLS 场景

证书通常由 `examples/test/scripts/gen_certs/` 生成，再由 `start_all_with_conf.sh` 传给 `test_server`。

## 什么时候才需要扩 `test_server`

只有这些情况才建议改 `examples/code/server/test_server.rs`：

- 现有端点无法表达你的断言需求
- 缺少某个协议级能力
- 现有 OIDC/auth/webhook/mirror 行为无法覆盖你的场景
- 你需要一个确定性的测试后端行为，而不是在线服务或 Docker 依赖

改之前，先在现有端点里搜一遍：

```bash
rg -n "route\\(|start_auth_server|mirror|oidc|verify|webhook" examples/code/server/test_server.rs
```
