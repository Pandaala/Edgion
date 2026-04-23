# Admin API HTTPS 与访问控制

## 背景

Edgion 有两个二进制会暴露 Admin HTTP API：

- **Controller**（`:5800`）：供 edgion-cli 和 K8s 探针使用，消费者均为非浏览器客户端，通常作为 K8s ClusterIP Service 部署，不对外暴露。
- **Center**（联邦中心）：将来会带前端（浏览器 Dashboard），需要对外或跨集群暴露，安全需求更高。

两者当前均以纯 HTTP 提供服务，共享同一套 `compose_admin_routes` + `login_handler`。登录成功后 `Set-Cookie` 没有 `Secure` 标志：

```rust
// src/core/common/local_auth/handlers.rs:146
let cookie_value = format!(
    "edgion_token={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
    token, expiry_secs
    // 缺少 "; Secure"
);
```

本文档记录将来需要做的三项工作，以彻底解决 Admin API 的传输安全和访问控制问题。

---

## 两者的差异化设计

| 维度 | Controller | Center |
|------|-----------|--------|
| 典型网络位置 | K8s 集群内，ClusterIP，不对外 | 对外或跨集群，带浏览器前端 |
| 主要消费者 | edgion-cli（Bearer token）、K8s 存活/就绪探针 | 浏览器 Dashboard + edgion-cli |
| 是否需要 HTTPS | 近期不需要（内网 + 无浏览器） | 需要（前端必须 HTTPS，否则 Secure Cookie 无法工作） |
| 是否需要 Secure Cookie | 不需要（无浏览器路径） | 需要（跟随 HTTPS 联动） |
| 主要防线 | allow_admin_ips（IP 白名单，不可伪造） | HTTPS + Secure Cookie + allow_admin_ips |

`Secure` 属性应**跟随 TLS 是否配置**自动联动，而不是无条件添加：
- HTTP 模式下加 `Secure` 是矛盾信号，且在非 localhost 的 HTTP 环境下会导致 Cookie 静默失效，难以排查。
- Center 配置了 `[server.admin_tls]` → 登录响应自动带 `; Secure`；Controller 不配 → 不带。

---

## Task 1：Center Admin API HTTPS 支持

### 目标

Center 的 Admin API 能以 HTTPS 提供服务，登录 Cookie 自动携带 `Secure` 标志。

### 所需依赖（已在 Cargo.toml，无需新增）

- `axum-server = { version = "0.8", features = ["tls-rustls"] }` — HTTPS 绑定
- `rustls = { version = "0.23", features = ["ring"] }` — TLS 库
- `rcgen = { version = "0.14.7", ... }` — 可选，未来自签证书生成

### 配置 Schema 变化

Center 配置（TOML）新增可选段，模式与 gRPC mTLS（`conf_sync_security.tls`）一致：

```toml
[server]
admin_listen = "0.0.0.0:5800"

[server.admin_tls]            # 可选，不配置则 HTTP
cert = "certs/admin/server.crt"   # PEM 服务端证书
key  = "certs/admin/server.key"   # PEM 服务端私钥
# ca = "certs/admin/ca.crt"       # 可选，配置后要求客户端证书（mTLS）
```

### 涉及文件

- `src/core/common/config/mod.rs` 或 Center 对应的 config 结构体 — 新增 `AdminTlsConfig`
- `src/core/center/cli/mod.rs`（Center 启动入口）— 读取 `admin_tls` 并传入 serve
- Center Admin API 的 serve 函数 — 根据 TLS 配置分支：
  - 有 `admin_tls` → `axum_server::tls_rustls::RustlsConfig` + `axum_server::Server::bind_rustls(...)`
  - 无 `admin_tls` → 当前路径 `tokio::net::TcpListener::bind` + `axum::serve`
- `src/core/common/local_auth/handlers.rs` — `login_handler` 和 `logout_handler` 接收 `secure_cookie: bool` 参数，有 TLS 时传 `true`

### Cookie 修改

```rust
// login_handler — secure_cookie 由调用方（Center serve 初始化）注入到 State 或通过参数传入
let secure = if secure_cookie { "; Secure" } else { "" };
let cookie_value = format!(
    "edgion_token={}; HttpOnly{}; SameSite=Strict; Path=/; Max-Age={}",
    token, secure, expiry_secs
);

// logout_handler
let secure = if secure_cookie { "; Secure" } else { "" };
let cookie_value = format!("edgion_token=; HttpOnly{}; SameSite=Strict; Path=/; Max-Age=0", secure);
```

`secure_cookie` 最干净的传递方式是放入 `UnifiedAuthState` 或单独的 Axum State 扩展，在 Center 启动时按 `admin_tls` 是否配置决定值。

### 注意事项

- Controller 不做 TLS，不改动，`secure_cookie` 始终为 `false`，行为与现在完全一致。
- Center 未来的 K8s 探针（liveness/readiness）走 HTTP 的 `/health`、`/ready` 路径，这两条在 `skip_paths` 里，但如果 Admin API 改成 HTTPS-only，探针的 `scheme` 需要同步改为 `HTTPS`。
- edgion-cli 连接 Center 时，`reqwest` 客户端需要能校验 server cert（通过 `ssl_verify` 配置或配置信任的 CA）。`ssl_verify` 字段已存在于 `AdminAuthConfig`，可复用类似机制。

---

## Task 2：Admin API allow_admin_ips 访问控制

### 目标

Controller 和 Center 的 Admin API 均支持 IP 白名单，在认证中间件之前拦截来自未授权 IP 的请求。这是 Controller（无 HTTPS）最重要的访问控制手段。

### 配置 Schema 变化（Controller 和 Center 共用）

```toml
[server]
admin_listen = "0.0.0.0:5800"
allow_admin_ips = [             # 可选，不配置则允许所有 IP
  "10.0.0.0/8",
  "127.0.0.1/32",
  "::1/128",
]
```

### 实现要点

1. **IP 来源必须是 TCP 连接的 `remote_addr`**，不能信任 `X-Forwarded-For` 或任何可伪造的 Header。Axum 通过 `axum::extract::ConnectInfo<std::net::SocketAddr>` 获取。

2. **中间件位置**：在 `compose_admin_routes` 的最外层，比 `unified_auth_middleware` 更早执行，甚至在 `skip_paths` 放行之前（健康探针路径通常来自集群内部，不应被 IP 白名单阻断，因此需要仔细考虑探针 IP 是否在白名单内，或者给探针路径单独豁免）。

3. **CIDR 匹配**：使用 `ipnet` crate（项目已有或可低代价引入）做 CIDR 解析和包含判断。

4. **未配置（allow_admin_ips 为空）时**：允许所有 IP 通过，行为与当前完全一致（向后兼容）。

5. **被拦截时**：返回 `403 Forbidden`，响应体 `{"success":false,"error":"Access denied"}`，不泄露认证状态。

### 涉及文件

- `src/core/common/config/mod.rs`（或各自 config 结构体）— 新增 `allow_admin_ips: Vec<String>`
- 新建 `src/core/common/api/ip_allowlist.rs`（或类似路径）— Axum middleware
- `src/core/common/api/compose.rs` 的 `compose_admin_routes` — 在最外层加 `.layer(ip_allowlist_middleware)`

### 探针豁免决策

健康探针（`/health`、`/ready`）来自 kubelet，IP 是节点 IP，不一定在 Pod 的内网 CIDR 里。两种方案：

- **方案 A**：IP 白名单中间件在路径匹配 `skip_paths` 之后才执行，探针路径被 skip_paths 提前放行，IP 白名单不影响探针。实现稍复杂（需要先判断路径再判断 IP）。
- **方案 B**：IP 白名单配置里 operator 自行把节点 CIDR 加进去。文档说明清楚即可。

推荐方案 A，对 operator 透明。

---

## Task 3：Controller HTTPS 支持（按需，未定）

Controller 当前定位是不对外暴露，无浏览器前端，因此近期无 HTTPS 需求。但如果将来：

- Controller 也要带轻量前端
- 需要跨集群访问 Controller Admin API

则 Task 1 的 Center HTTPS 实现可以直接复用到 Controller，代码路径完全相同，只需要 Controller 配置文件里加 `[server.admin_tls]` 段并提供证书。

**当前不实现，留口子即可**——`AdminTlsConfig` 结构体在 Task 1 做成公共的放进 `src/core/common/config/`，Controller 的 serve 函数预留 TLS 分支（可以用 `#[allow(dead_code)]` 暂时标记），待需求明确后解锁。

---

## 实施顺序建议

```
Task 1（Center HTTPS）
  优先级：高（前端上线前必须完成）
  工作量：中（~200-300 行，含测试）
  依赖：无

Task 2（allow_admin_ips）
  优先级：高（Controller 当前唯一有效的网络访问控制）
  工作量：小（~100-150 行，含测试）
  依赖：无，可与 Task 1 并行

Task 3（Controller HTTPS）
  优先级：低（按需）
  工作量：小（Task 1 完成后复用）
  依赖：Task 1
```

---

## 关联

- 安全审计原始 finding：`tasks/security-audit/04-admin-api.md` — H2
- 安全审计总览状态：`tasks/security-audit/00-overview.md` — H2 条目
