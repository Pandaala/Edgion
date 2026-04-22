# Admin 凭证引导（Auth Bootstrap）

描述 Controller / Center 的 admin 凭证在启动时的来源，以及 operator 的配置路径。

## 行为总览

| 组件 | 无 `[local_auth]` + 无 `[auth]` | 有 `[local_auth]` | 有 `[auth]` OIDC |
|------|---|---|---|
| Controller | 自动生成 Secret `edgion-admin-credentials`（K8s 模式）或写入 conf 目录（本地模式）；登录用生成的密码 | Static 凭证从配置文件 | OIDC 流程 |
| Center | **server 启动 + 业务路由永远 503** + 启动 WARN 日志 | Static 凭证从配置文件 | OIDC 流程 |

**关键差异**：Controller 支持"零配置启动"（Secret 自动生成保留）；Center 要求 operator 显式提供凭证。

### Center 的 503 语义（fail-close）

Center 没有自动生成能力。当 `[local_auth]` 和 `[auth]` 都未配置时：
- `require_auth` 派生为 `true`（safe default：避免 admin API 裸奔）
- 没有任何 auth provider ready
- `unified_auth_middleware` 走"fail-close"分支返回 `503 Service Unavailable`（携带 `Retry-After: 10`）
- `/health`、`/ready`、`/metrics`、`/api/v1/auth/status` 仍可访问（skip_paths + early-return 路径）
- 启动日志打印 WARN：`No authentication configured. Admin endpoints will return 503 until you configure [local_auth] or [auth] in your TOML.`

## Controller: 读取自动生成的凭证

### K8s 模式

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
    -o jsonpath='{.data.password}' | base64 -d
```

### 本地模式

```bash
cat {conf_dir}/Secret_edgion-system_edgion-admin-credentials.yaml
```

### 修改密码

`kubectl edit secret edgion-admin-credentials` 后 Controller 的 secret retry loop 会热更新
`UnifiedAuthState`（CancellationToken 关停信号下语义稳定；见 fix #01 round 3 的结构性回归）。

## Center: 如何提供 admin 凭证

### 选项 A：`local_auth` 本地账号密码（YAML）

```yaml
local_auth:
  enabled: true
  username: admin
  # bcrypt hash。生成：htpasswd -bnBC 10 "" <your-password> | tr -d ':\n'
  # 或使用明文密码字段（deploy/center-test/deploy.yaml 有 test 示例）
  password: "<your-password>"
  jwt_secret: "<一个足够长的随机字符串，至少 32 字符>"
  jwt_expiry_hours: 24
  skip_paths:
    - "/health"
    - "/ready"
```

### 选项 B：`auth` OIDC

```yaml
auth:
  enabled: true
  discovery: "https://your-oidc-provider/.well-known/openid-configuration"
  client_id: "..."
  # ...
```

### 什么都不配的后果（fail-close via 503）

Center 启动依然成功，但：
- 所有 `/api/v1/*` 业务路径以 503 拒绝
- `/health` / `/ready` / `/metrics` / `/api/v1/auth/status` 仍可访问
- 启动日志打印 WARN（见上）

operator 发现后补配后重启即可。

### 显式关闭所有 provider 的后果（no-auth mode，**危险**）

如果 operator **明确**把所有 provider 都设成 `enabled: false`，例如：

```yaml
local_auth:
  enabled: false
auth:
  enabled: false
```

此时：
- `require_auth = false`（respect operator 的 opt-out 意图）
- `unified_auth_middleware` 直接 passthrough 所有请求
- **所有 `/api/v1/*` 业务路径无需 token 即可访问**——admin API **完全开放**
- 启动时 WARN：`Admin API is running in NO-AUTH mode: ...`（2026-04-20 起）

这是合法的"纯无认证模式"（例如内网隔离 + 其它网关层做认证），但**非常容易被误操作触发**（比如 operator 想"临时关掉 local_auth 只用 OIDC"却同时关错了两个）。如果你的部署真的要走这条路径，请：

1. 显式在部署文档写明"无认证模式，依赖 X 层做认证"
2. 不要把这个 YAML 放到能被外网访问的 Center 上

如果你只是想**切换**到 OIDC-only，正确做法是：
- `local_auth` 段整个**删掉**（或只留一个 `enabled: false`）
- `auth` 段配成 `enabled: true` + 有效 discovery
- 这样 `oidc_intent = true`，`require_auth = true`，走正常 OIDC 流程

## K8s 部署：Secret 挂载 Center YAML

推荐模式：将 Center YAML 配置（含 `local_auth:` 段）存为 K8s Secret，挂载到 Center Pod 作为主配置文件。

参考：`deploy/center-test/deploy.yaml`。

## JWT 撤销语义（`[local_auth]` 模式）

**现状**：本地 admin JWT 是 **stateless** 的——Edgion 无服务端 token 黑名单，`POST /api/v1/auth/logout` 仅清除浏览器 cookie（`Max-Age=0`），**已签发的 token 在到达 `exp` 之前仍然有效**。

暴露窗口上限 = `jwt_expiry_hours`（默认 `24`）。

### 这是刻意的 stateless 取舍

与 **Argo CD 本地管理员** / **Kubernetes Dashboard 内置 token** 同模式。引入服务端黑名单会遇到两种架构困境：

| 方案 | 问题 |
|---|---|
| 内存黑名单（`HashSet<jti>`） | 多实例 HA 部署时每实例独立——logout 打到实例 A、后续请求打到实例 B → B 不知道撤销 = **假象防御**；进程重启清零 |
| Redis 黑名单 | Admin API 路径引入 Redis 硬依赖 + 每请求 1 次 RT 延迟 + Redis 故障时 fail-open vs fail-close 两难 |

所以 Edgion 选择"stateless JWT + 通过其他途径提供撤销能力"。

### 需要真正撤销能力时：**走 OIDC federation**

`unified_auth` 支持 `[auth]` 段配 OIDC（Keycloak / Auth0 / Okta / Azure AD / Google 等）：
- 在 IdP 端撤销用户 session 或 refresh token → 所有基于该 session 的 access token 失效
- IdP 端统一的 session 生命周期管理、revocation、审计日志
- 这是"管理端认证"真正需要生产级撤销时的**业界标准路径**

切换示例（Controller 或 Center 任一）：

```yaml
# 关闭 local_auth，改走 OIDC
local_auth:
  enabled: false
auth:
  enabled: true
  discovery: "https://idp.example.com/.well-known/openid-configuration"
  client_id: "edgion-admin"
  client_secret_ref:
    name: oidc-admin-secret
  # ...
```

### 如果必须用 `[local_auth]`：缩短暴露窗口

对暂时不能走 OIDC 的部署：

```yaml
local_auth:
  enabled: true
  # ...
  jwt_expiry_hours: 4       # 生产推荐 1-4 小时，而非默认 24h
```

**关于 CLI 工作流**：`edgion-cli` 的 `ensure_auth()` 会在 token 过期或 controller 重启后**自动 re-login**（前提是配置文件里能解析出 credentials），缩短 `jwt_expiry_hours` 不会破坏 CLI 用户体验；只会让浏览器 dashboard 场景更频繁提示重登。

### 退役运维的标准操作（"撤销"的替代方案）

由于没有 per-token 撤销，处理 operator 离职 / 权限回收时：

1. **改密码**：`kubectl edit secret edgion-admin-credentials` 修改 `password` 字段 → Controller secret retry loop 热更新 `UnifiedAuthState` → 所有未来登录必须用新密码（但**已签发的 token** 仍到 `exp` 才失效）
2. **轮换 `jwt_secret`**：修改 Secret 中的 `jwt_secret` 字段 → 所有已签发 token 立刻失效（**一键 revoke all**，但会顺便把当前登录的所有管理员都踢掉）
3. **等窗口过期**：若 `jwt_expiry_hours` 已缩到 1-4h，这也是可接受的方案

**方案 2（轮换 jwt_secret）是最快的"批量撤销"**，适合紧急情况；日常人员变动建议配合 OIDC federation 做。

## 相关回归测试

- `examples/test/scripts/integration/test_center_no_auth.sh` —
  Center 无 auth 配置启动 + /api/v1/controllers 业务路径 503 + WARN 日志的集成层结构性回归。
  纳入 `run_integration.sh` 的前置检查。

## 历史

- 2026-04-20：Center 删除 `admin/edgion1234` 默认 fallback（BREAKING）。
  - 核心原因：fix #01 round 2 bug 1（`LocalAuthConfig::default()` 启动 panic）的根因是
    Center 的 `or_else(|| Some(edgion1234))` fallback 替 operator 造了一个
    password/jwt_secret 不满足 fix #01 加强 validate 的凭证。
  - 修复：Center 不再兜底；operator 必须显式配置 `[local_auth]` 或 `[auth]`，
    否则 admin API 全部 503。
  - 相关 spec：`docs/superpowers/specs/2026-04-20-admin-server-unification-design.md`。
