# 15 - External Plugin Protocol (外部插件协议)

**优先级**: P2 - 锦上添花

## 问题

Edgion 有 Forward Auth（外部认证），但缺少通用的外部处理协议。非 Rust 开发者无法用自己熟悉的语言编写插件，限制了社区扩展性。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | 多语言 Plugin Runner（Java、Go、Python），通过 Unix Socket RPC 通信 |
| Kong | Plugin Server（Go、JavaScript、Python），通过 IPC 调用 |
| Envoy | `ext_proc`（gRPC 外部处理）、`ext_authz`（gRPC/HTTP 外部鉴权）、WASM（Proxy-Wasm SDK，支持 Rust/Go/C++/AssemblyScript） |

## 需要实现

- [ ] 定义外部插件 gRPC 协议（request/response 交换格式）
- [ ] ext_proc 类型：外部进程处理请求/响应的 header 和 body
- [ ] 外部插件的生命周期管理（连接池、超时、熔断）
- [ ] 多阶段介入（request_filter / upstream_response_filter）
- [ ] SDK 模板（至少 Go 和 Python）
- [ ] WASM 插件支持（Proxy-Wasm 规范，可选）

## 备注

Envoy 的 ext_proc 是最成熟的参考设计。可先实现 gRPC 协议的 ext_proc，再考虑 WASM。
