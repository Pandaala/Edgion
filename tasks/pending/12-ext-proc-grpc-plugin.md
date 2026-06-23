# 12 - ext_proc gRPC External Processor Plugin (独立的 gRPC 外部处理插件)

**优先级**: P2 - 锦上添花
**关联**: tasks/todo/gap-analysis/15-external-plugin-protocol.md（本文是 15 的一个独立子任务，聚焦 ext_proc）

> **实现状态（feature-05-22）**：核心 + 集成测试均已实现，代码未 commit。
> - **单元测试通过**：config 校验 12 项（含 target host:port 强校验、IPv6、body 模式拒绝）、header mutation/pseudo-header 守卫/keep_empty_value 7 项。
> - **已落地**：trimmed-but-wire-compatible 的 `envoy.service.ext_proc.v3` proto（`edgion-common/src/ext_proc/`，client+server codegen）、`ExtProcConfig` schema、gateway `ext_proc` 插件（request header via `RequestFilter` + response header via async `UpstreamResponse`，两阶段 ImmediateResponse→`ErrResponse` 短路且 gRPC-route 感知、`failureModeAllow`、verdict 超时保护、多值响应头保真、HTTP/2 伪 header 守卫）、CRD `requestPlugins` enum 加 `ExtProc`。
> - **集成测试已写并跑通（6/6 通过）**：`test_server.rs` 加了 fake ext_proc gRPC server（`--ext-proc-port 30041`）；conf 在 `edgion-tests/integration/conf/EdgionPlugins/ExtProc/`（basic / failclose / failopen / coexist）；suite `edgion-tests/src/client/suites/edgion_plugins/ext_proc/`（6 用例：req+resp header、403 短路+自定义头、fail-close 500+body、fail-open 放行+确认未注入头、**坏 verdict→fail-close 500**、ForwardAuth+ExtProc 共存）；已接入 `test_client.rs` 与 `run_integration.sh`。回归：ResponseRewrite / Cors / KeyAuth（同样走 ExtensionRef / ErrResponse）仍通过。
> - **集成测试覆盖说明（诚实记录）**：核心相位 + 两种 fail-mode + 坏 verdict fail-close + 共存均端到端验证。**单元层覆盖**（非集成）：走私防护拒绝集（hop-by-hop 头在集成层会被 pingora 自身剥离，无法隔离验证，故单元测才是正确层级）、body 模式 schema 拒绝、多值 Set-Cookie collect 保真、timeout 上限。**未覆盖（可接受/后续）**：到 processor 的 TLS、ext_proc 配置热更新、gRPC route（非 HTTP route）、processor 收头后挂起的超时行为。
> - **经过一轮 subagent 深入 review 并修复**：target 弱校验导致静默 fail-open、verdict 可能无限挂起、多值响应头被折叠、请求阶段 ImmediateResponse 非 gRPC 感知等。
> - **跑集成时发现并修复的关键 bug**：ExtensionRef 的 route 级 fan-out 此前只注册 request / 同步 upstream_response_filter / response_body 三个阶段，**漏了异步 `UpstreamResponse` 阶段**（在 ext_proc 之前从没有插件用这个阶段，所以一直没接线）。已给 `ExtensionRefFilter` 补 `impl UpstreamResponse` 并在 `add_from_httproute_filters_inner`（HTTP+gRPC 两处）注册 `add_upstream_response`。否则经 ExtensionRef 引用的 EdgionPlugins 的响应头处理永不执行。
> - **第二轮 subagent review 又修了**：(1) ExtensionRef 异步阶段对 dangling-ref 用「直接写响应+ErrTerminateRequest」在响应阶段会被上游响应覆盖（响应阶段 runner 只认 ErrResponse 短路）→ 改为先 `resolve_runtime_silent` 预解析、无 async 插件就 GoodNext 早退（顺带消除每个 ExtensionRef 响应的无谓查表/日志），dangling → `ErrResponse{500}`，并给 `run_extension_async` 的 Err 臂按 stage 分支兜底 TOCTOU。(2) 请求阶段 `ErrResponse` 构建器先算 content-length 再 drain 队列头，ext_proc 的 ImmediateResponse 现在能注入 content-length/transfer-encoding 造成 framing 错位 → 改为 drain 后再重置 framing（对齐响应阶段 `build_response_local_reply` 的做法）。
> - **第三轮 subagent review（含安全视角）又修了**：(1) **[安全] 请求/响应头 mutation 只挡了伪 header**，外部 processor 能往上游请求注入 `Content-Length`/`Transfer-Encoding`/`Connection` 等 → **请求/响应走私**。`mutation.rs` 加 `is_proxy_managed_header` 拒绝集（hop-by-hop/framing），set/append/remove 一律拒绝（对齐 Envoy `disallow_system_headers`）。(2) **[正确性] processor 返回错相位/空 verdict 命中 `_ => GoodNext` 静默 fail-open**，即使配了 fail-close → 抽出 `fail_mode()`，gRPC 错误与坏 verdict 都按 `failure_mode_allow` 决策（坏 verdict 还加 `tracing::warn`）。(3) `timeoutMs` 加上限 60s；ExtensionRef 异步早退补 `set_refer_to` 让访问日志正确标记。框架改动（请求阶段 framing 重置）经独立 review 确认对所有 ErrResponse 插件无回归。
> - **构建注意**：本机默认 profile（debuginfo=2）编译 gateway/controller 会 OOM；用 `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=off` 才能编出二进制跑集成。
> - **文档已补**：skills 目录 `skills/02-features/03-resources/13-edgion-plugins.md`（插件目录加 `ExtProc` + 更新 `UpstreamResponse` 阶段说明）；user-guide `docs/en` 与 `docs/zh-CN` 的 `.../edgion-plugins/ext-proc.md`（含范围/配置/受保护 header 安全说明/失败处理/限制/FAQ）+ 两个 README 索引。`validate_agent_docs.py` 通过。
> - **全部完成**，无剩余 TODO（代码 + 集成测试 + 文档；未 commit）。

---

## 0. 一句话目标

新增一个**独立的 HTTP 插件 `ext_proc`**，让 Gateway 通过 **gRPC 双向流**把请求/响应的各阶段交给外部服务处理（读/改 header、改响应 body、放行/短路）。

**明确边界：这不是给 forward_auth 加 gRPC 开关，而是一个和 forward_auth 平级、职责独立的新插件。** 两者可以共存（先 forward_auth 鉴权，再 ext_proc 处理）。

### 为什么必须独立而非并入 forward_auth

| 维度 | forward_auth | ext_proc |
|------|------|------|
| 语义 | 鉴权裁决（放行/拒绝的布尔决策） | 通用处理管道（读/改 header、body、响应） |
| 传输 | 短连接 HTTP 一问一答 | long-lived 双向 gRPC 流 + 连接池 |
| 介入相位 | 仅 request 阶段 | request + response 多相位 |
| 配置 schema | auth_url / allowed_headers / fail 策略 | grpc_service / 每阶段 SEND·SKIP / body 模式 / 各阶段超时 |

混在一起会让一个叫 "auth" 的插件干改 body 的事，命名与职责错位，且把 gRPC 流连接池的复杂度污染到干净的 HTTP 鉴权路径上。

---

## 1. 参考设计：Envoy ext_proc（最成熟的范本）

> 强烈建议直接对标 Envoy，**proto 尽量兼容 `envoy.service.ext_proc.v3`**，这样现有的各语言 ext_proc 服务端实现/SDK 可以直接复用，比自定义协议价值大得多。

### 1.1 协议本体

- gRPC 服务：`envoy.service.ext_proc.v3.ExternalProcessor`，方法 `Process`，是一条**双向流（bidirectional streaming）**。
- **只支持 gRPC**，没有 HTTP 变体（区别于 ext_authz 的 gRPC/HTTP 双 transport）——因为要流式逐块传 body。
- 注意区分两个 "gRPC"：
  - **控制通道**（Gateway ↔ 外部插件）= gRPC，固定。
  - **被代理的业务流量** = 任意 HTTP，不要求是 gRPC。ext_proc 是把 HTTP 流量的各阶段「用 gRPC 包装」喂给外部服务。

### 1.2 阶段模型（Envoy 把一次事务拆成多段）

```
RequestHeaders  → RequestBody(可分块) → RequestTrailers
ResponseHeaders → ResponseBody(可分块) → ResponseTrailers
```

每个阶段：Gateway 发 `ProcessingRequest`，外部返回 `ProcessingResponse`，可执行：
- header mutation（add/remove/set）
- body mutation（替换 body）
- `ImmediateResponse`（直接短路，例如返 403，不再回源）
- 改路由 / cluster（高级，Edgion 可暂不做）

### 1.3 关键可配置项（务必照搬这套粒度）

- **`processing_mode`**：每个阶段独立设 `SEND` / `SKIP`。默认只 SEND request header、SKIP 其它，按需开启，省开销。
- **body 模式**：`NONE` / `STREAMED`（流式分块）/ `BUFFERED`（整包，需缓存）/ `BUFFERED_PARTIAL`。
- **`failure_mode_allow`**：外部服务挂掉时 fail-open（放行）还是 fail-close（拒绝）。
- 各阶段 message timeout。

---

## 2. Edgion 现状约束（决定哪些功能本期不做）⚠️

### 2.1 当前插件相位（`edgion-gateway/src/plugins/runtime/traits/`）

| Edgion 相位 | trait | 对应 Envoy 阶段 |
|------|------|------|
| 请求阶段 | `RequestFilter::run_request`（async） | RequestHeaders |
| 响应 header（async） | `UpstreamResponse::run_upstream_response` | ResponseHeaders |
| 响应 header（sync） | `UpstreamResponseFilter`（同步，**不能 await，不可用于 gRPC**） | ResponseHeaders |
| 响应 body | `UpstreamResponseBodyFilter`（**同步 + 只读**，返回 `Option<Duration>` 仅限速，**不能改 body**） | ResponseBody |

> ⚠️ **实现修正（与初稿不符）**：响应 header 必须走**异步**的 `UpstreamResponse` trait，而非初稿写的同步 `UpstreamResponseFilter`（后者无法 `await` gRPC）。响应 body 的 `UpstreamResponseBodyFilter` 是同步且**只读**的（只能限速、无法替换 body），所以 ext_proc 改写 response body 在当前架构下不可行，本期不做。

### 2.2 ⛔ 不支持 request body 缓存 / 没有 request body filter 相位

**Edgion 目前没有通用的 request body 插件 hook，也不缓存/不缓冲 request body。** 因此下列 Envoy ext_proc 能力**本期明确不实现**：

- ❌ `RequestBody` 阶段（读 request body）
- ❌ `RequestBody` BUFFERED 模式
- ❌ 改写 request body
- ❌ 基于 request body 内容做决策/路由
- ❌ `RequestTrailers`

> request_mirror 插件虽然能 tee request body（`session.rs` 的 mirror state + mpsc channel），但那是「旁路复制」，不等于「缓存后回放给插件决策」。**不要据此以为 request body 可读改。** 真要支持需要先做一个独立的 request body buffering 基础设施（属于另一个更大的任务，本期不碰）。

### 2.3 ✅ 本期可支持的能力子集（MVP）

| 能力 | 支持 | 落在哪个相位 |
|------|:---:|------|
| 读 request header | ✅ | RequestFilter |
| 改 request header（加/删/改，发往上游） | ✅ | RequestFilter |
| 请求阶段短路返回（ImmediateResponse，如 403） | ✅ | RequestFilter |
| 读/改 response header | ✅ | `UpstreamResponse`（async） |
| 响应阶段短路返回（ImmediateResponse） | ✅ | `UpstreamResponse`（映射为 local reply，状态+body） |
| —— 读/改 response body（流式） —— | ❌ | （body filter 同步只读，无法改写，见 2.1） |
| —— 读/改 request body —— | ❌ | （无相位，见 2.2） |

也就是说：**本期 ext_proc = request header 处理 + response header 处理**（含两阶段的 ImmediateResponse 短路）。这已经覆盖大量真实场景（统一改 header、按响应 header 做后处理、外部裁决短路），且与现有架构零冲突。response body 改写需要先扩展 body filter 相位（异步 + 可变），属于后续独立任务。

---

## 3. 需要实现（按优先级）

### 必做（MVP）
- [ ] 定义 / 引入 ext_proc proto（**优先直接用 `envoy.service.ext_proc.v3`**；若裁剪也要保持 message 语义兼容，便于复用生态）
- [ ] 新增独立 HTTP 插件 `ext_proc`，目录 `edgion-gateway/src/plugins/http/ext_proc/`（参照 `trace_context` 插件的骨架）
- [ ] Gateway 作为 **gRPC client** 连外部处理服务（复用现有 tonic 基础设施，参考 conf_sync/fed_sync）
- [ ] 接入相位：`RequestFilter`（request header + 短路）、`UpstreamResponseFilter`（resp header）、`UpstreamResponseBodyFilter`（resp body 流式）
- [ ] 配置 schema：`grpc_service`（target/超时/TLS）、`processing_mode`（每阶段 SEND/SKIP）、`failure_mode_allow`（fail-open/close）
- [ ] **在 `config/crd/edgion-crd/edgion_plugins_crd.yaml` 的 plugin `type` enum 里加上 `ext_proc`**（见第 5 节坑）
- [ ] header mutation 应用逻辑（把外部返回的 add/remove/set 落到实际请求/响应）
- [ ] ImmediateResponse → Edgion 的短路返回机制映射

### 选做
- [ ] response body 的 BUFFERED 模式（response body 可缓冲，相对可行）
- [ ] 连接池 / 熔断 / 背压细化
- [ ] Go / Python SDK 模板 + 示例外部处理服务

### 明确不做（本期）
- ❌ 所有 request body 相关能力（见 2.2，缺基础设施）
- ❌ 改路由 / cluster 选择
- ❌ WASM / Proxy-Wasm（属于 gap-analysis 15 的另一条腿）

---

## 4. 配置示例（草案，供 schema 设计参考）

```yaml
plugins:
  - type: ext_proc
    grpc_service:
      target: "ext-proc.svc.cluster.local:9000"
      timeout_ms: 200
      tls: false
    processing_mode:
      request_header_mode:   SEND   # 读/改 request header
      request_body_mode:     SKIP   # ⛔ 本期强制 SKIP（无 request body 支持）
      response_header_mode:  SEND
      response_body_mode:    STREAMED   # NONE | STREAMED | BUFFERED
    failure_mode_allow: false        # 外部挂掉时 fail-close
```

> schema 校验里应**硬性拒绝 `request_body_mode: STREAMED/BUFFERED`**（或直接不暴露该字段），避免用户配了一个静默不生效的功能。

---

## 5. 踩过 / 会踩的坑 ⚠️

1. **CRD type enum 必须手改**（高频坑，已被记录）：新增插件类型后，**一定要手动在 `config/crd/edgion-crd/edgion_plugins_crd.yaml` 的 `type` enum 里加 `ext_proc`**，否则 Controller 校验阶段直接 reject 资源，Gateway 永远收不到配置。这是加任何新插件类型的固定动作。

2. **不要被 request_mirror 误导**：它能 tee request body 不代表 request body 可被插件读改决策。本期 request body 一律不可用（见 2.2）。

3. **静默失效最坑**：如果让用户能配 request_body_mode 却不生效，会非常误导。要么 schema 拒绝，要么文档+校验显式报错。

4. **fail-open vs fail-close 要在 schema 里显式选**，不能默认。鉴权类用 ext_proc 时默认 fail-close 更安全；纯改 header 场景可 fail-open。

5. **gRPC 是 client 不是 server**：Gateway 这边是去**连**外部服务（client），别和 Controller↔Gateway 那条 Gateway 当 server 的链路搞混。

6. **双向流的连接生命周期**：long-lived 流 + 背压，比一问一答的 HTTP 复杂。MVP 阶段可以先做「每请求一条短流」简化，性能优化（连接池/复用）放选做。

7. **header mutation 的大小写 / 伪 header**：HTTP/2 伪 header（`:status` 等）和普通 header 的 mutation 规则不同，照 Envoy 语义处理，别让外部服务能改 `:method` 之类造成路由错乱。

---

## 6. 参考文件 / 上手路径

- 插件骨架范本：`edgion-gateway/src/plugins/http/trace_context/`（最近新增的插件，结构最干净）
- 插件相位 trait：`edgion-gateway/src/plugins/runtime/traits/{request_filter,upstream_response_filter,upstream_response_body_filter}.rs`
- forward_auth（对照「外部 HTTP 调用」现状）：`edgion-gateway/src/plugins/http/forward_auth/`
- 流式 body 处理参考：`edgion-gateway/src/plugins/http/request_mirror/plugin.rs`、`response_rewrite/`
- gRPC client 基础设施：conf_sync / fed_sync（`edgion-common` 的 proto codegen + tonic）
- 插件注册 / store：`edgion-gateway/src/plugins/http/plugin_store.rs`、`mod.rs`
- CRD enum：`config/crd/edgion-crd/edgion_plugins_crd.yaml`
- 插件开发指南：`skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`

---

## 7. 验收

- [ ] 集成测试：起一个最小 ext_proc 外部服务（加一个 response header / 改 request header / 触发 403 短路），验证三个相位生效
- [ ] 验证 `failure_mode_allow` 两种行为（外部服务关闭时 fail-open 放行 / fail-close 拒绝）
- [ ] 验证配 request_body_mode 时被 schema 正确拒绝（不静默吞掉）
- [ ] forward_auth 与 ext_proc 同时挂载互不干扰
