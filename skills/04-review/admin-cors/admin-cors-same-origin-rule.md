# Admin API CORS：同源部署，不挂 CORS 层

## 结论

Edgion 的 Admin API（Controller `:5800` / Center `:5810`）**不挂** CORS 层。这是**架构决策**，不是缺失配置。review / 审计若出现"Admin API CORS `allow_origin(Any)` 过于开放 / 应加白名单"类 finding，应按"非代码修复"处理，除非能指出新出现的跨域浏览器消费者。

## 原因

Admin API 的实际消费者与其部署形态决定 CORS 在当前架构下无受益人：

- **`edgion-cli`**（Rust reqwest 客户端）：不做 CORS 检查
- **Prometheus `/metrics` 抓取**：服务端对服务端，不涉及浏览器 CORS
- **K8s `/health` `/ready` 探针**：不涉及浏览器 CORS
- **`edgion-dashboard`（浏览器 UI，仓库 `ws3/edgion-dashboard`）**：
  - dev：Vite `server.proxy` 将 `/api`、`/health` 代理至 `localhost:5810`，浏览器视为同源（见 `edgion-dashboard/vite.config.ts`）
  - prod：dashboard 构建产物嵌入 Controller 二进制，与 Admin API 同源（见 `edgion-dashboard/README.md` "生产环境"段）

跨域浏览器消费者在当前部署形态下为 0；挂 CORS 层（无论 permissive 还是 allowlist）都是为不存在的场景保留代码与配置表面。

## 判定规则

当 review / 审计报告出现以下模式时，按"非代码修复"关闭：

1. "Admin API CORS `allow_origin(Any)` / permissive"（H1 历史原文）
2. "应引入 `[admin].cors_allowed_origins` 配置限定允许来源"
3. "生产模式应强制校验跨域来源白名单"
4. "Admin API Cookie/Bearer 可通过跨域请求被恶意网页窃取"

验证四点再关闭：

- `src/core/common/api/mod.rs` 不含 `CorsLayer` / `tower_http::cors::*` 的引用
- `src/core/common/api/compose.rs` 的装配顺序不含 "CORS layer" 步骤
- `Cargo.toml` 不含 `tower-http` 依赖（或含但不启用 `cors` feature）
- `edgion-dashboard/vite.config.ts` 中 `server.proxy` 对 `/api` / `/health` 仍正确转发至后端（确认 dev 仍走同源）

四点满足 → 非真实问题，指向 `tasks/security-audit/04-admin-api.md` 的 H1 FIXED（删除 CORS 层）决策，关闭。

## 例外：何时本结论不适用

**唯一例外**：如果未来引入**独立 origin 部署**的浏览器消费者（例如 dashboard 从 Controller 剥离、独立域名部署，或出现第三方 web 工具集成 Admin API），本结论失效，需要重新设计——届时引入**精确来源白名单**（不是 `Any`），且白名单内容应来自真实部署需求。

在此变化发生之前，任何"加 CORS 配置字段"的建议应先被要求证明存在跨域浏览器消费者。

## 背景记录

- 决策日期：2026-04-21
- 决策来源：`tasks/security-audit/` H1 条目
- 相关设计文档：`docs/superpowers/specs/2026-04-21-h1-admin-cors-removal-design.md`
- 相关实现记录：`docs/superpowers/plans/2026-04-21-h1-admin-cors-removal.md`
- 同类模式：`skills/04-review/auth-timing/basic-auth-username-enumeration.md`（架构性关闭，拒绝代码修复建议）
