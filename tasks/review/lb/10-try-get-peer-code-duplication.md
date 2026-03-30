# [P3] try_get_peer 中大量重复代码

## 严重程度定义（本文档约定）

| 级别 | 含义 |
|------|------|
| **P0** | 数据丢失 / 进程崩溃 |
| **P1** | 功能异常 / 内存泄漏（含长期资源无法回收） |
| **P2** | 性能问题（延迟、吞吐、CPU） |
| **P3** | 代码质量、可维护性 |

**本文档归类：P3** — 当前重复**不直接导致**线上故障或性能回退，但会显著增加演进成本与漏改风险；属于**可维护性与一致性**类技术债。

---

## 问题描述

`try_get_peer` 根据 `EdgionService` 分支解析上游并构造 `HttpPeer`。其中 **`Service`、`ServiceEndpoint`、`ServiceEndpointSlice`** 三条路径在「拿到 `Backend` 之后」的逻辑高度同构：

- 克隆 `lb_addr`、基于 `br_port` 与 `PortLookupSource` 调用 `resolve_target_port` 得到 `peer_addr`
- `extract_tls_config`、（测试指标场景下）`extract_hash_key` 写入 `ctx.hash_key`
- 向 `ctx.get_current_upstream_mut()` 写入 `backend_addr`、`lb_backend_addr`、`service_key`、`lb_policy`、`tls`（`BackendTlsInfo` 结构体字面量完全一致）
- `create_tls_peer` + `apply_backend_app_protocol`

**真正不同的只有两点**：

1. **后端来源**：`select_backend_by_policy` / `select_from_endpoints` / `select_from_endpoint_slice`
2. **`resolve_target_port` 的 `PortLookupSource`**：`Auto` / `Endpoint` / `EndpointSlice`

此外，`Service` 分支在上述公共块之前还有 **Session Persistence**（`sp_target_addr`）：若命中则构造单节点 `Backend` 并做健康检查，否则走 `select_backend_by_policy`；**命中 SP 之后**与另外两条路径仍共享同一套 upstream / peer 流水线。

**次要重复**：`ServiceClusterIp` 与 `ServiceExternalName` 在解析出 `SocketAddr` 后，`validate_backend_addr`、`extract_tls_config` + TLS 记录、`create_tls_peer`、`apply_backend_app_protocol` 的结构也高度相似；二者与前三条路径的差异在于**没有** `lb_addr` / `resolve_target_port` / LB 策略 upstream 字段，但 **peer 侧 TLS 与 app_protocol 应用**仍是复制粘贴形态。

**小不一致**：`Service` / `ServiceEndpoint` / `ServiceEndpointSlice` 在 `upstream.tls` 上采用**内联** `BackendTlsInfo { ... }`；`ServiceClusterIp` / `ServiceExternalName` 使用已有辅助函数 `record_tls_to_upstream`。语义接近，但实现路径不统一，重构时若只改一处易遗漏。

---

## 影响范围

| 区域 | 说明 |
|------|------|
| **可维护性** | 新增 upstream 元数据（如 TLS `protocol`、自定义 header、tracing 字段）需在多个 `match` 臂中同步修改 |
| **行为一致性** | TLS / metrics / upstream 上下文任一逻辑微调时，易出现「某一 kind 漏改」导致 HTTP 与 gRPC、或不同 `kind` 行为分叉 |
| **评审成本** | PR 中大量重复 diff，Reviewer 难以快速确认「是否所有分支等价」 |
| **测试** | 若未对每种 `EdgionService` 做等价性覆盖，重构或复制错误可能长期潜伏 |

---

## 代码位置

| 符号 | 文件 | 行号（约） |
|------|------|------------|
| `try_get_peer` | `Edgion/src/core/gateway/backends/mod.rs` | 756–1031 |
| `EdgionService` | 同上 | 157–176 |
| `PortLookupSource`、`resolve_target_port` | 同上 | 261–366 |
| `select_backend_by_policy`、`select_from_endpoints`、`select_from_endpoint_slice` | 同上 | 437–535 |
| `extract_tls_config`、`create_tls_peer`、`apply_backend_app_protocol` | 同上 | 537–719、220–238 |
| `record_tls_to_upstream` | 同上 | 721–736 |

核心重复段（`Service` 分支在 SP/LB 之后）：

```830:863:Edgion/src/core/gateway/backends/mod.rs
            let lb_addr = backend.addr.clone();
            let mut peer_addr = lb_addr.clone();
            resolve_target_port(&mut peer_addr, br_port, &service_key, PortLookupSource::Auto);

            let (use_tls, sni) = extract_tls_config(&backend_tls_policy);

            let lb_policy_clone = lb_policy.clone();

            if ctx.gateway_info.metrics_test_type.is_some()
                && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
            {
                let hash_key_bytes = extract_hash_key(session, lb_policy);
                ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
            }

            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.backend_addr = Some(peer_addr.clone());
                upstream.lb_backend_addr = Some(lb_addr);
                upstream.service_key = Some(service_key.clone());
                upstream.lb_policy = lb_policy_clone;

                if use_tls {
                    upstream.tls = Some(crate::types::BackendTlsInfo {
                        sni: if sni.is_empty() { None } else { Some(sni.clone()) },
                        handshake_ok: None,
                        protocol: None,
                        cipher: None,
                    });
                }
            }

            let mut peer = create_tls_peer(peer_addr, &backend_tls_policy, gateway_key_ref)?;
            apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
            Ok(peer)
```

`ServiceEndpoint`（958–992）与 `ServiceEndpointSlice`（995–1029）与上述块相比，仅 **backend 选择函数** 与 **`PortLookupSource` 枚举变体** 不同，其余行级结构一致。

---

## 重复代码分析（对比表格）

### 表 1：`Service` vs `ServiceEndpoint` vs `ServiceEndpointSlice`

| 步骤 | Service | ServiceEndpoint | ServiceEndpointSlice | 是否重复 |
|------|---------|-----------------|----------------------|----------|
| 前置：Service 存在性 | `contains` 检查 | （隐含在 select 内） | （隐含在 select 内） | Service 独有 |
| 前置：Session Persistence | `sp_target_addr` 分支 | 无 | 无 | Service 独有 |
| 选择 Backend | `select_backend_by_policy` | `select_from_endpoints` | `select_from_endpoint_slice` | **差异点** |
| `lb_addr` / `peer_addr` / `resolve_target_port` | ✓ `Auto` | ✓ `Endpoint` | ✓ `EndpointSlice` | **仅 source 不同** |
| `extract_tls_config` | ✓ | ✓ | ✓ | 相同 |
| metrics 测试 + `ctx.hash_key` | ✓ | ✓ | ✓ | 相同 |
| `upstream` 五字段 + `tls` 字面量 | ✓ | ✓ | ✓ | 相同 |
| `create_tls_peer` + `apply_backend_app_protocol` | ✓ | ✓ | ✓ | 相同 |

**粗算**：`ServiceEndpoint` 与 `ServiceEndpointSlice` 两个分支各约 35 行，其中约 **25+ 行**与 `Service` 的 LB 后路径逐行等价；`Service` 整条 arm 更长主要因为 SP 与 `contains` 前置逻辑。

### 表 2：`ServiceClusterIp` vs `ServiceExternalName`（peer 侧共性）

| 步骤 | ServiceClusterIp | ServiceExternalName | 备注 |
|------|------------------|---------------------|------|
| 从 `Service` 取地址字段 | `cluster_ip` | `external_name` | 差异 |
| 端口解析（br_port / spec） | 相同模式 | 相同模式 | 重复 |
| `format!` + `parse` + `validate_backend_addr` | ✓ | ✓ | 重复 |
| `extract_tls_config` + `record_tls_to_upstream` | ✓ | ✓ | 重复 |
| `create_tls_peer` + `apply_backend_app_protocol` | ✓ | ✓ | 重复 |

### 表 3：LB 类路径 vs ClusterIP / ExternalName（upstream 上下文）

| 能力 | Service / Endpoint / EndpointSlice | ClusterIP / ExternalName |
|------|-----------------------------------|---------------------------|
| `lb_backend_addr` | 有 | 无（无 LB 选中地址） |
| `resolve_target_port` | 有 | 无 |
| `lb_policy` 写入 upstream | 有 | 无 |
| `backend_addr` + TLS peer | 有 | 有（地址来源不同） |

因此公共抽取时建议分层：**「LB 解析后的 Backend + PortLookupSource」**一条管线，**「直连型 SocketAddr（ClusterIP / ExternalName）」**另一条管线，二者在 **TLS 记录 + `create_tls_peer` + app_protocol** 上仍可共享。

---

## 修改建议

1. **抽取「LB 选中 Backend → Peer」统一函数**（名称示例：`build_peer_from_lb_backend`）  
   - 入参：`backend: Backend`、`br_port`、`service_key`、`PortLookupSource`、`lb_policy`、`backend_tls_policy`、`gateway_key_ref`、`session`、`ctx`、`backend_app_protocol` 等  
   - 内联现有：`lb_addr`/`peer_addr`、`resolve_target_port`、`extract_tls_config`、metrics `hash_key`、`upstream` 赋值、`create_tls_peer`、`apply_backend_app_protocol`  
   - 三个分支退化为：**解析出 `backend` + 选对 `PortLookupSource`** 后调用该函数。

2. **抽取 upstream 上下文写入**（名称示例：`fill_upstream_from_lb_selection`）  
   - 将 `backend_addr`、`lb_backend_addr`、`service_key`、`lb_policy`、`tls`（或统一改为调用 `record_tls_to_upstream` + 扩展其参数以覆盖 `BackendTlsInfo` 全字段）集中在一处，避免三处结构体字面量漂移。

3. **Session Persistence 作为前置包装**  
   - `Service` 分支只负责：`contains` →（可选）`sp_target_addr` 得到 `Backend` 或 `select_backend_by_policy` → 调用统一 `build_peer_from_lb_backend(..., PortLookupSource::Auto)`。

4. **ClusterIP / ExternalName**  
   - 抽取 `resolve_direct_service_addr(service, br_port, /* ClusterIp | ExternalName */) -> Result<SocketAddr, EdgionStatus>` 或共享「端口解析 + 字符串地址」逻辑；  
   - `validate_backend_addr` 之后统一调用小型 `build_peer_from_socket_addr(ctx, addr, ...)`，内部 `record_tls_to_upstream` + `create_tls_peer` + `apply_backend_app_protocol`。

5. **可选：Builder 模式**  
   - 若后续 peer 选项持续增多（mTLS、SNI、连接池 group_key、hook 等已在 `create_tls_peer` 内），可考虑 `PeerBuildContext` 聚合参数，但**优先**用纯函数抽取减少 `match` 体积即可，避免过度抽象。

---

## 重构示例（示意代码）

以下仅为**结构示意**，非可直接粘贴的完整实现；命名与可见性需与模块内风格对齐。

### 1. 统一 LB 路径

```rust
fn fill_upstream_lb_context(
    ctx: &mut EdgionHttpContext,
    peer_addr: &SocketAddr,
    lb_addr: &SocketAddr,
    service_key: &str,
    lb_policy: Option<ParsedLBPolicy>,
    use_tls: bool,
    sni: &str,
) {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.backend_addr = Some(peer_addr.clone());
        upstream.lb_backend_addr = Some(lb_addr.clone());
        upstream.service_key = Some(service_key.to_owned());
        upstream.lb_policy = lb_policy;
        if use_tls {
            record_tls_to_upstream(ctx, true, sni); // 或扩展为写入完整 BackendTlsInfo
        }
    }
}

fn maybe_record_consistent_hash_metrics(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    lb_policy: &Option<ParsedLBPolicy>,
) {
    if ctx.gateway_info.metrics_test_type.is_some()
        && matches!(lb_policy, Some(ParsedLBPolicy::ConsistentHash(_)))
    {
        let hash_key_bytes = extract_hash_key(session, lb_policy);
        ctx.hash_key = String::from_utf8(hash_key_bytes).ok();
    }
}

fn build_peer_from_lb_backend(
    ctx: &mut EdgionHttpContext,
    session: &Session,
    backend: pingora_load_balancing::Backend,
    br_port: Option<i32>,
    service_key: &str,
    port_source: PortLookupSource,
    lb_policy: &Option<ParsedLBPolicy>,
    backend_tls_policy: &Option<Arc<BackendTLSPolicy>>,
    gateway_key_ref: Option<&str>,
    backend_app_protocol: Option<BackendAppProtocol>,
) -> Result<Box<HttpPeer>, EdgionStatus> {
    let lb_addr = backend.addr.clone();
    let mut peer_addr = lb_addr.clone();
    resolve_target_port(&mut peer_addr, br_port, service_key, port_source);

    let (use_tls, sni) = extract_tls_config(backend_tls_policy);
    let lb_policy_clone = lb_policy.clone();

    maybe_record_consistent_hash_metrics(ctx, session, lb_policy);
    fill_upstream_lb_context(
        ctx,
        &peer_addr,
        &lb_addr,
        service_key,
        lb_policy_clone,
        use_tls,
        &sni,
    );

    let mut peer = create_tls_peer(peer_addr, backend_tls_policy, gateway_key_ref)?;
    apply_backend_app_protocol(&mut peer, backend_app_protocol, ctx);
    Ok(peer)
}
```

### 2. `match` 臂收缩示意

```rust
EdgionService::ServiceEndpoint => {
    let backend = select_from_endpoints(&service_key, lb_policy, session)?;
    build_peer_from_lb_backend(
        ctx,
        session,
        backend,
        br_port,
        &service_key,
        PortLookupSource::Endpoint,
        lb_policy,
        &backend_tls_policy,
        gateway_key_ref,
        backend_app_protocol,
    )
}
// Service / ServiceEndpointSlice 同理，仅 port_source 与 select_* 不同
```

### 3. `Service` + Session Persistence

`Service` 分支在调用 `build_peer_from_lb_backend` 之前保留现有逻辑即可：`contains` 校验 → 若 `ctx.sp_target_addr` 有值则构造 `Backend` 并走 `is_backend_healthy`（不健康时返回与现实现一致的 `EdgionStatus`）→ 否则 `select_backend_by_policy`。得到 `backend` 后统一传入 `PortLookupSource::Auto`，不再复制后续 upstream / peer 代码块。

---

## 小结

- **根因**：`try_get_peer` 按 `EdgionService` 扁平展开，**后端选择**与 **PortLookupSource** 本应作为参数化的两步，被复制成三段（及 Service 的 SP 前置）几乎相同的实现。  
- **风险**：演进时漏改分支、TLS/upstream 语义分叉、评审困难。  
- **方向**：以 **`build_peer_from_lb_backend` + upstream 填充辅助函数** 为主，SP 与 ClusterIP/ExternalName 分别保留薄包装；必要时再引入 Builder。  

实施重构后建议：**针对各 `EdgionService::from_kind` 与 SP 路径补充或回归现有集成测试**，确认 upstream 字段与 peer 行为与重构前一致。
