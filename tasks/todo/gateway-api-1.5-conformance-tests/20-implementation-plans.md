# Gateway API v1.5.0 Conformance — 缺失功能实现方案

共 8 个功能点，涉及 25 个 conformance 测试。按优先级排列。

---

## P1: HTTPRoute Multiple Mirrors (1 test)

**测试**: `HTTPRouteRequestMultipleMirrors`
**问题**: 当前单 rule 只支持一个 mirror，最后一个 `set_mirror_state()` 覆盖前面的。
**工作量**: 小 (~1 天)

### 根因

`ctx.mirror_state: Option<MirrorState>` 只能存一个。多个 `RequestMirrorPlugin` 依次执行，后者覆盖前者。

### 改动

| 文件 | 改动 |
|------|------|
| `src/types/ctx.rs` | `mirror_state: Option<MirrorState>` → `mirror_states: Vec<MirrorState>` |
| `src/core/gateway/plugins/runtime/traits/session.rs` | `set_mirror_state()` → `add_mirror_state()` (push 语义) |
| `src/core/gateway/plugins/runtime/session_adapter.rs` | `self.ctx.mirror_state = Some(state)` → `self.ctx.mirror_states.push(state)` |
| `src/core/gateway/plugins/http/request_mirror/plugin.rs` | 调用改为 push 语义 |
| `src/core/gateway/routes/http/proxy_http/pg_request_body_filter.rs` | **核心改动**: body tee 从单 mirror 改为遍历 `Vec<MirrorState>`，每个 mirror 独立发送 body chunk，失败的单独 abandon |
| `src/core/gateway/routes/http/proxy_http/pg_logging.rs` | `ctx.mirror_state.take()` → `ctx.mirror_states.drain(..)` |

### 测试
- 添加双 mirror 的 HTTPRoute YAML
- 验证两个 mirror backend 都收到请求
- primary backend 正常响应客户端

---

## P2: TLSRoute Terminate 完善 (3 tests)

**测试**: `TLSRouteTerminateSimpleSameNamespace`, `TLSRouteListenerTerminateNotSupported`, `TLSRouteListenerMixedTerminationNotSupported`
**问题**: Terminate 实现已存在于 `proxy.rs`，但未充分验证和测试。
**工作量**: 中 (~3 天)

### 现有实现

- `EdgionTlsTcpProxy` (proxy.rs): 使用 Pingora TLS listener 终止 TLS，读 SNI 匹配 TLSRoute，代理解密后的字节到明文后端
- `EdgionTlsPassthroughProxy` (passthrough_proxy.rs): 纯 TCP listener，解析 ClientHello 提取 SNI，原样透传

### 需要完善

1. **端到端测试验证**: 确保 Terminate 模式下后端收到明文 TCP (`BackendIsTLS: false`)，后端看不到 SNI
2. **Listener status 反映支持的模式**: `supportedKinds` 和 listener condition 正确报告 Terminate 支持状态
3. **"不支持"测试**: 如果某个 listener 不支持 Terminate/Mixed，status 中反映 `Accepted=False, reason=UnsupportedTLSMode`

### 改动

| 文件 | 改动 |
|------|------|
| `handlers/gateway.rs` | `compute_supported_kinds()` 根据 TLS mode 返回正确的 supportedKinds；不支持的 mode 设置 `Accepted=False` |
| `src/core/gateway/routes/tls/proxy.rs` | 验证 Terminate 流程完整性：TLS 终止 → SNI 匹配 → 明文代理 |
| 集成测试 | 新增 TLSRoute Terminate 端到端测试 |

---

## P3: TLSRoute Mixed Termination (1 test)

**测试**: `TLSRouteMixedTerminationSameNamespace`
**问题**: 同端口无法同时有 Passthrough 和 Terminate 两个 listener。
**工作量**: 大 (~5 天)

### 架构设计

Pingora 每端口只能绑定一个 socket。Mixed 模式必须使用**单个 plain TCP listener**，在应用层根据 SNI 决定是透传还是终止：

```
Client → [TCP Listener] → Parse ClientHello → Extract SNI
                              ↓
                    Match TLSRoute + Listener Mode
                         /              \
                   Passthrough        Terminate
                   (relay raw)    (app-level TLS handshake
                                   → plaintext proxy)
```

### 新增组件

**`EdgionTlsMixedProxy`** (新文件 `mixed_proxy.rs`):
1. 读 ClientHello，提取 SNI
2. 查 TlsRouteTable，返回 `(route, TlsTerminationMode)`
3. Passthrough → 原样透传（复用 passthrough 逻辑）
4. Terminate → 应用层 TLS 握手（`tokio_boring::SslStream::new()`），然后明文代理

### 改动

| 文件 | 改动 |
|------|------|
| `src/types/resources/tls_route.rs` | 新增 `TlsTerminationMode` enum + `resolved_tls_mode` skip 字段 |
| `src/core/gateway/routes/tls/gateway_tls_routes.rs` | route table 携带 mode: `Vec<(Arc<TLSRoute>, TlsTerminationMode)>` |
| `handlers/gateway.rs` | 解析 parentRef 时传播 listener 的 tls.mode 到 route |
| `src/core/gateway/routes/tls/mixed_proxy.rs` | **新文件**: `EdgionTlsMixedProxy` ServerApp |
| `src/core/gateway/routes/tls/common.rs` | **新文件**: 提取 relay/backend 共享逻辑 |
| `listener_builder.rs` | 新增 `add_tls_mixed_listener()`；检测同端口不同 mode 时自动启用 Mixed proxy |

### 最难点
应用层 TLS 握手：在已读取 ClientHello 的 raw TCP stream 上手动完成 TLS 握手。需要用 `boring`/`openssl` API 创建 `SslAcceptor`，加载 SNI 对应证书，包装为 `SslStream`。

---

## P4: Gateway Frontend Client Certificate Validation (4 tests)

**测试**: `GatewayFrontendClientCertificateValidation`, `...InsecureFallback`, `...InvalidDefault`, `...InvalidFrontend`
**问题**: Gateway listener 无 `tls.frontendValidation` 字段，不支持入口 mTLS。
**工作量**: 大 (~5 天)

### 新类型

```rust
// gateway.rs
pub struct FrontendTLSValidation {
    pub ca_certificate_refs: Option<Vec<ObjectReference>>,
    pub optional: Option<bool>,  // true = 允许无客户端证书
}

// 添加到 GatewayTLSConfig:
pub frontend_validation: Option<FrontendTLSValidation>,
pub resolved_frontend_ca_certs: Option<Vec<String>>,  // skip, 运行时填充
```

### Controller 侧

| 文件 | 改动 |
|------|------|
| `src/types/resources/gateway.rs` | 新增 `FrontendTLSValidation`、`ObjectReference` 结构体 |
| `handlers/gateway.rs` parse() | 解析 `ca_certificate_refs`：从 ConfigMap/Secret 获取 CA PEM，注册 ref tracking |
| `handlers/gateway.rs` validate | 新增错误变体：`InvalidCACertRef`, `CACertNotFound`, `CACertRefNotPermitted` |
| ConfigMap ref tracking | 新增 `ConfigMapRefManager`（仿 `SecretRefManager` 模式）用于级联 requeue |

### Gateway 侧

| 文件 | 改动 |
|------|------|
| `runtime/matching/tls.rs` | `GatewayTlsEntry` 新增 `frontend_ca_certs`, `frontend_validation_optional` |
| `tls/runtime/gateway/tls_pingora.rs` | 新增 `configure_gateway_frontend_mtls()`：创建 X509Store，设置 `SslVerifyMode::PEER` (+`FAIL_IF_NO_PEER_CERT` if !optional) |
| `tls_pingora.rs` apply_gateway_tls_cert() | 调用 `configure_gateway_frontend_mtls()` |

### 已有可复用代码
`configure_mtls()` (EdgionTls 的 mTLS) 已有完整的 BoringSSL CA 加载和 verify mode 设置逻辑，可直接复用。

---

## P5: Gateway Infrastructure (1 test, Provisional)

**测试**: `GatewayInfrastructure`
**问题**: Gateway 无 `spec.infrastructure` 字段。
**工作量**: 中 (~2 天)

### 分析

Edgion 是预部署的单 binary 网关，不像 Operator 那样为每个 Gateway 创建 Pod/Service。实现方式：

1. **解析字段**: 添加 `infrastructure` 到 `GatewaySpec`
2. **标签传播**: 如果 Edgion 管理自己的 Service 对象，patch 上去；否则仅在 status 中确认接受

### 改动

| 文件 | 改动 |
|------|------|
| `src/types/resources/gateway.rs` | 新增 `GatewayInfrastructure` struct (labels, annotations, parametersRef) |
| `handlers/gateway.rs` | parse() 无需特殊处理；on_change() 可选 patch Service 标签 |

---

## P6: Gateway Backend TLS Configuration (2 tests)

**测试**: `GatewayInvalidTLSBackendConfiguration`, `GatewayBackendClientCertificateFeature`
**问题**: Gateway 无 `spec.tls.backend.clientCertificateRef`，当前仅通过 BackendTLSPolicy 实现。
**工作量**: 中 (~3 天)

### 新类型

```rust
// gateway.rs
pub struct GatewaySpecTLS {
    pub backend: Option<GatewayBackendTLS>,
}
pub struct GatewayBackendTLS {
    pub client_certificate_ref: Option<SecretObjectReference>,
}
```

### 改动

| 文件 | 改动 |
|------|------|
| `src/types/resources/gateway.rs` | 新增 `GatewaySpecTLS`, `GatewayBackendTLS` |
| `handlers/gateway.rs` parse() | 解析 client cert Secret ref，注册 SecretRefManager |
| `handlers/gateway.rs` validate | 无效 ref → `ResolvedRefs=False, reason=InvalidClientCertificateRef` |
| `pg_upstream_peer.rs` | 无 BackendTLSPolicy 时，fallback 到 Gateway 级 client cert |
| `listener_builder.rs` | 将 Gateway backend TLS config 传入 ListenerContext |

### 优先级关系
Gateway client cert 是**默认值**，BackendTLSPolicy 可逐 backend **覆盖**。

---

## P7: Gateway Static/Optional Addresses (2 tests)

**测试**: `GatewayStaticAddresses`, `GatewayOptionalAddressValue`
**问题**: 大部分已实现，可能需要补充 status condition。
**工作量**: 小 (~0.5 天)

### 现有实现

`derive_gateway_addresses()` (gateway.rs:487-520) 已处理：
- `spec.addresses` 非空 → 复制到 `status.addresses`
- 否则 → Service ClusterIP → 默认地址 → `0.0.0.0`

### 可能缺失

1. `Programmed` condition 是否正确反映地址分配结果
2. 声明 `GatewayStaticAddresses` 和 `GatewayAddressEmpty` 为 supported features

### 改动

| 文件 | 改动 |
|------|------|
| `handlers/gateway.rs` update_status() | 确认 `Programmed` condition 反映地址状态 |
| conformance test feature list | 注册 supported features |

---

## P8: ListenerSet 全套 (10 tests)

**测试**: 10 个，全部是 v1.5 新增
**问题**: 完全未实现，需要新 CRD + 全链路开发。
**工作量**: 很大 (~11 天)

### 架构决策

**在 Controller 侧合并**: ListenerSet listeners 在 controller parse 时合并到 Gateway 的有效 listener 列表中。Gateway 同步到 data plane 时已包含完整 listener 集合。Gateway 侧改动最小。

### 实现阶段

#### Phase 1: 类型和注册 (~1 天)

| 文件 | 改动 |
|------|------|
| `src/types/resources/listener_set.rs` | **新文件**: `ListenerSetSpec` (parentRef + listeners), `ListenerSetStatus` |
| `src/types/resources/gateway.rs` | 新增 `AllowedListeners` (namespaces.from: None/Same/Selector/All) |
| `src/types/resource/kind.rs` | 新增 `ListenerSet = 21` |
| `src/types/resource/defs.rs` | 注册 ListenerSet |
| `storage.rs` | 添加 API 映射: `gateway.networking.k8s.io/v1/ListenerSet` |
| CRD 安装脚本 | 更新 v1.5 CRD 安装 |

#### Phase 2: ListenerSet Handler (~2 天)

新文件: `handlers/listener_set.rs`

- `filter()`: 无 class 过滤
- `parse()`:
  1. lookup_gateway() 找父 Gateway
  2. 检查 `gateway.spec.allowedListeners.namespaces.from` (None→拒绝, Same→同NS, Selector→标签匹配)
  3. 跨命名空间 ReferenceGrant 检查
  4. TLS Secret 解析（复用 Gateway 的模式）
  5. ListenerPortManager 注册
- `on_change()`: requeue 父 Gateway
- `on_delete()`: 清理所有 ref tracking，requeue Gateway
- `update_status()`: 设置 Accepted/Programmed condition + 每个 listener 的 status

#### Phase 3: Gateway Handler 合并 (~2 天)

| 改动 | 说明 |
|------|------|
| parse() 末尾 | 查找引用此 Gateway 的所有 ListenerSet，合并 listener 到 `merged_listeners` |
| 冲突检测 | ListenerSet listener 参与 ListenerPortManager，与 Gateway 自身 listener 检测冲突 |
| status | 新增 `attached_listener_sets` 计数 |
| on_change() | Gateway `allowedListeners` 变更时 requeue 所有引用它的 ListenerSet |

新增全局索引: `gateway_listenerset_index` (gateway_key ↔ listenerset_key 双向映射)

#### Phase 4: Route 附加到 ListenerSet (~2 天)

| 文件 | 改动 |
|------|------|
| `route_utils.rs` validate_parent_ref_accepted() | 支持 `kind: ListenerSet` parentRef |
| `hostname_resolution.rs` | ListenerSet parentRef 的 hostname 解析 |
| 各 route handler | HTTPRoute/GRPCRoute/TLSRoute 的 parentRef 验证支持 ListenerSet |

**关键规则** (来自 conformance test):
- Route parentRef → Gateway (无 sectionName) → 在 Gateway 所有 listener + 所有 ListenerSet listener 上生效
- Route parentRef → Gateway (有 sectionName) → 仅在 Gateway 自身匹配的 listener 上生效
- Route parentRef → ListenerSet → 仅在该 ListenerSet 的 listener 上生效
- Route parentRef → ListenerSet (有 sectionName) → 仅在该 ListenerSet 匹配的单个 listener 上生效

#### Phase 5: Gateway 侧 (~1 天)

| 文件 | 改动 |
|------|------|
| `conf_sync` | 新增 `ClientCache<ListenerSet>` |
| `runtime/server/base.rs` | `configure_listeners()` 遍历 `merged_listeners` 创建物理 listener |
| `api/mod.rs` | Admin API 新增 ListenerSet 端点 |

#### Phase 6: 跨命名空间 + ReferenceGrant (~1 天)

- ListenerSet → Gateway 跨 NS: 需要 ReferenceGrant
- ListenerSet 的 TLS Secret 跨 NS: 需要 ReferenceGrant
- ReferenceGrant 变更 → requeue 相关 ListenerSet

#### Phase 7: 测试 (~2 天)

- 10 个 conformance test 的 YAML manifest
- 集成测试覆盖所有场景

---

## 总结

| # | 功能 | 测试数 | 工作量 | 依赖 |
|---|------|--------|--------|------|
| P1 | Multiple Mirrors | 1 | ~1 天 | 无 |
| P2 | TLSRoute Terminate 完善 | 3 | ~3 天 | 无 |
| P3 | TLSRoute Mixed | 1 | ~5 天 | P2 |
| P4 | Frontend Client Cert | 4 | ~5 天 | 无 |
| P5 | Gateway Infrastructure | 1 | ~2 天 | 无 |
| P6 | Gateway Backend TLS | 2 | ~3 天 | 无 |
| P7 | Gateway Addresses | 2 | ~0.5 天 | 无 |
| P8 | ListenerSet | 10 | ~11 天 | 无 |
| | **总计** | **24** | **~30.5 天** | |

P1/P2/P7 可并行开发，P3 依赖 P2。其余互相独立。
