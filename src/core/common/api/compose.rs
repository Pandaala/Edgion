//! Admin HTTP router composition.
//!
//! 提供 `compose_admin_routes` —— Controller 和 Center 唯一共享的装配入口。
//! 函数内部把 business 路由、共享 auth 端点（login/me/logout/status）、
//! metrics、health/ready、unified_auth middleware 按固定顺序装配。调用方
//! 拿到的 `Router` 视为终态，追加路由会绕过 middleware—— 见函数 doc 的终态
//! 约定。
//!
//! Admin API 不挂 CORS 层：本地调试时由 edgion-dashboard 的 Vite `server.proxy`
//! 处理（见 `edgion-dashboard/vite.config.ts`），生产由 dashboard 嵌入
//! Controller 同源部署。如未来出现独立 origin 部署需求，再引入精确来源白名单。

use std::sync::Arc;

use axum::Router;

use super::{add_auth_status_route, add_local_auth_routes, wrap_with_unified_auth_layer};
use crate::core::common::unified_auth::UnifiedAuthState;

/// 按固定顺序装配 admin HTTP Router。
///
/// # 装配顺序（函数内写死，调用方无法影响）
/// 1. `business`（各组件自己的业务路由）
/// 2. `/api/v1/auth/{login, logout, me}`（共享 local_auth handler，
///    仅当 `local_auth_intent = true` 时挂载）
/// 3. `/api/v1/auth/status`（无条件挂载）
/// 4. `unified_auth` middleware（包所有路由；skip_paths 由 `auth_state` 提供）
///
/// # 终态约定
/// **返回的 `Router` 视为终态**。不要再 `.route()` / `.layer()`——任何追加
/// 都会绕过 auth middleware。新增 admin
/// 路由应放入 `business` 参数后再调用本函数。
///
/// # 参数
/// - `business`: 各组件私有业务路由。**不应**包含本函数装配的 auth 路由
///   （重复 merge 会触发 axum panic）。业务路由自己挂 `/health` / `/ready`
///   / `/metrics` 由调用方决定；本函数不再额外挂载。
/// - `auth_state`: 共享认证状态，来自 `UnifiedAuthState::from_configs`。
/// - `local_auth_intent`: 是否意图启用 local auth。`true` 时挂载
///   `/auth/{login,logout,me}`；`false` 时省略（OIDC-only 或显式禁用）。
///   注：此参数不从 `auth_state` 派生。原因是 Controller 的 K8s Secret 自动
///   生成路径允许"意图启用但 provider 尚未就绪"（`auth_state.local` 为 `None`
///   但 intent 为 `true`），这种状态下仍应挂载路由（handler 返回 503 直到
///   Secret 就绪）。`auth_state.local.load().is_some()` 无法区分这种情况
///   和"显式禁用"，所以保留独立的 intent 参数。
pub fn compose_admin_routes(business: Router, auth_state: Arc<UnifiedAuthState>, local_auth_intent: bool) -> Router {
    let with_auth_routes = add_local_auth_routes(business, auth_state.clone(), local_auth_intent);
    let with_status = add_auth_status_route(with_auth_routes, auth_state.clone());
    wrap_with_unified_auth_layer(with_status, auth_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    use crate::core::common::local_auth::LocalAuthConfig;

    fn valid_local() -> LocalAuthConfig {
        LocalAuthConfig {
            username: "admin".to_string(),
            password: "a_long_enough_password_123".to_string(),
            jwt_secret: "a_long_enough_jwt_secret_value_abcdef".to_string(),
            ..LocalAuthConfig::default()
        }
    }

    fn new_state_with_local() -> Arc<UnifiedAuthState> {
        let cfg = valid_local();
        UnifiedAuthState::from_configs(None, Some(&cfg), true, "test").unwrap()
    }

    fn dummy_business() -> Router {
        Router::new().route("/foo", get(|| async { "foo" }))
    }

    /// T1 — compose 后 /auth/{login,logout,me,status} 均可路由到（非 404）。
    #[tokio::test]
    async fn compose_mounts_all_shared_routes() {
        let state = new_state_with_local();
        let app = compose_admin_routes(dummy_business(), state, true);

        for path in &[
            "/api/v1/auth/login",
            "/api/v1/auth/logout",
            "/api/v1/auth/me",
            "/api/v1/auth/status",
        ] {
            let req = Request::builder().uri(*path).body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_ne!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "route {} should be mounted, got {:?}",
                path,
                resp.status()
            );
        }
    }

    /// T2 — /auth/me 无 Bearer token 时必须 401（不是 200、500、503）。
    #[tokio::test]
    async fn me_requires_auth() {
        let state = new_state_with_local();
        let app = compose_admin_routes(dummy_business(), state, true);
        let req = Request::builder().uri("/api/v1/auth/me").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// T3 — /auth/status 无 token 返回 200（status 端点不应被 middleware 挡）。
    #[tokio::test]
    async fn status_does_not_require_auth() {
        let state = new_state_with_local();
        let app = compose_admin_routes(dummy_business(), state, true);
        let req = Request::builder()
            .uri("/api/v1/auth/status")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// T4 — 业务路由 /foo 无 token 时 401（require_auth=true, 有 local provider）。
    #[tokio::test]
    async fn business_route_requires_auth_when_enabled() {
        let state = new_state_with_local();
        let app = compose_admin_routes(dummy_business(), state, true);
        let req = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// T5 — require_auth=false 时业务路由可直通（无 OIDC 无 local）。
    #[tokio::test]
    async fn business_route_passes_when_require_auth_false() {
        let state = UnifiedAuthState::from_configs(None, None, false, "test").unwrap();
        let app = compose_admin_routes(dummy_business(), state, false);
        let req = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
