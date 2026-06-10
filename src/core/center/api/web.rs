//! Static hosting for the edgion-dashboard web UI (Center only).
//!
//! The dashboard is served as a **public SPA fallback** mounted *after* the
//! auth-composed admin router (see `crate::core::center::cli`). Because the
//! fallback is added after `compose_admin_routes` returns its final (auth-wrapped)
//! router, it is not covered by the `unified_auth` middleware — which is exactly
//! what we want: the login page and its JS/CSS must load before authentication,
//! while every registered `/api/...` route stays auth-protected. The fallback only
//! ever receives paths that did not match a registered admin route.
//!
//! ## Asset source (resolved once at startup, highest precedence first)
//! 1. `EDGION_WEB_DIR` env var → serve that filesystem directory.
//! 2. `web.dir` config key → serve that filesystem directory.
//! 3. embedded assets (`embed-dashboard` feature) → serve from the binary.
//!
//! When none apply (no dir configured and the feature is off) the server runs in
//! pure-API mode and no fallback is installed.

use std::path::{Component, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[cfg(feature = "embed-dashboard")]
#[derive(rust_embed::RustEmbed)]
#[folder = "web/dashboard/"]
struct EmbeddedAssets;

/// SPA entry document served for any unmatched non-asset route.
const INDEX_HTML: &str = "index.html";

/// Resolved source of dashboard assets.
pub enum WebSource {
    /// Serve from a filesystem directory (`web.dir` or `EDGION_WEB_DIR`).
    Dir(PathBuf),
    /// Serve assets embedded in the binary (`embed-dashboard` feature).
    #[cfg(feature = "embed-dashboard")]
    Embedded,
}

impl WebSource {
    /// Resolve the asset source per precedence: `EDGION_WEB_DIR` > `config_dir`
    /// (the `web.dir` value) > embedded. Returns `None` when no UI is available
    /// (no directory configured and the `embed-dashboard` feature is off).
    pub fn resolve(config_dir: Option<&str>) -> Option<Self> {
        if let Ok(dir) = std::env::var("EDGION_WEB_DIR") {
            if !dir.is_empty() {
                return Some(WebSource::Dir(PathBuf::from(dir)));
            }
        }
        if let Some(dir) = config_dir {
            if !dir.is_empty() {
                return Some(WebSource::Dir(PathBuf::from(dir)));
            }
        }
        #[cfg(feature = "embed-dashboard")]
        {
            Some(WebSource::Embedded)
        }
        #[cfg(not(feature = "embed-dashboard"))]
        {
            None
        }
    }

    /// Load a single asset by its relative path (no leading slash). Returns the
    /// raw bytes when the asset exists, `None` otherwise.
    fn load(&self, rel: &str) -> Option<Vec<u8>> {
        match self {
            WebSource::Dir(root) => {
                let safe = sanitize_rel(rel)?;
                let full = root.join(safe);
                std::fs::read(full).ok()
            }
            #[cfg(feature = "embed-dashboard")]
            WebSource::Embedded => EmbeddedAssets::get(rel).map(|f| f.data.into_owned()),
        }
    }
}

/// Reject path-traversal and absolute components, returning a relative `PathBuf`
/// safe to join against the asset root. Returns `None` if the path escapes.
fn sanitize_rel(rel: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for comp in PathBuf::from(rel).components() {
        match comp {
            Component::Normal(c) => out.push(c),
            // Anything that could escape the root (`..`, `/`, drive prefixes) is rejected.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
            Component::CurDir => {}
        }
    }
    Some(out)
}

/// Map a file extension to a Content-Type. Kept dependency-free (works with or
/// without the `embed-dashboard` feature) and covers the asset types a Vite
/// build emits.
fn content_type_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Cache-Control value for an asset path.
///
/// - Vite emits content-hashed files under `assets/` → safe to cache immutably.
/// - The SPA shell (`index.html`) must always be revalidated so a new build is
///   picked up → `no-cache`.
/// - Everything else gets a short cache.
fn cache_control_for(rel: &str) -> &'static str {
    if rel == INDEX_HTML {
        "no-cache"
    } else if rel.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    }
}

fn asset_response(rel: &str, body: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type_for(rel))),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache_control_for(rel))),
        ],
        Body::from(body),
    )
        .into_response()
}

/// Serve a request that fell through to the dashboard fallback.
///
/// Resolution:
/// - `/api/...` (or `/api`) that reached the fallback is an unknown API route →
///   `404` (never the SPA shell, to avoid masking API mistakes).
/// - An exact asset hit (file exists) → that file.
/// - Otherwise → the SPA shell (`index.html`) so client-side routes resolve on a
///   hard refresh. If the shell is missing too → `404`.
pub async fn serve(source: Arc<WebSource>, uri: Uri) -> Response {
    let path = uri.path();

    // Unknown API paths must not be shadowed by the SPA shell.
    if path == "/api" || path.starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { INDEX_HTML } else { rel };

    if let Some(body) = source.load(rel) {
        return asset_response(rel, body);
    }

    // SPA fallback: serve the shell for client-side routes.
    match source.load(INDEX_HTML) {
        Some(body) => asset_response(INDEX_HTML, body),
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_traversal() {
        assert!(sanitize_rel("../etc/passwd").is_none());
        assert!(sanitize_rel("a/../../b").is_none());
        assert!(sanitize_rel("/abs").is_none());
    }

    #[test]
    fn sanitize_allows_normal() {
        assert_eq!(sanitize_rel("assets/app.js").unwrap(), PathBuf::from("assets/app.js"));
        assert_eq!(sanitize_rel("./index.html").unwrap(), PathBuf::from("index.html"));
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type_for("assets/app-abc123.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for("assets/style.css"), "text/css; charset=utf-8");
        assert_eq!(content_type_for("logo.svg"), "image/svg+xml");
        assert_eq!(content_type_for("weird.bin"), "application/octet-stream");
    }

    #[test]
    fn cache_control_rules() {
        assert_eq!(cache_control_for("index.html"), "no-cache");
        assert!(cache_control_for("assets/app-abc123.js").contains("immutable"));
        assert_eq!(cache_control_for("favicon.ico"), "public, max-age=3600");
    }

    #[tokio::test]
    async fn unknown_api_path_is_404_not_spa() {
        let dir = std::env::temp_dir().join("edgion_web_test_api404");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("index.html"), b"<html>shell</html>").unwrap();
        let src = Arc::new(WebSource::Dir(dir.clone()));

        let resp = serve(src.clone(), "/api/v1/does-not-exist".parse().unwrap()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn spa_fallback_serves_shell_for_client_route() {
        let dir = std::env::temp_dir().join("edgion_web_test_spa");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("index.html"), b"<html>shell</html>").unwrap();
        let src = Arc::new(WebSource::Dir(dir.clone()));

        // Unknown non-API route → SPA shell (200, text/html).
        let resp = serve(src.clone(), "/routes/list".parse().unwrap()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert_eq!(ct, "text/html; charset=utf-8");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Security invariant: the static fallback mounted AFTER `compose_admin_routes`
    /// is public (assets/SPA load without auth) while registered `/api` routes stay
    /// auth-protected, and unknown `/api` paths 404 instead of leaking the SPA shell.
    /// This locks the auth-ordering contract the whole design depends on.
    #[tokio::test]
    async fn fallback_is_public_while_api_stays_protected() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use axum::routing::get;
        use axum::Router;
        use tower::ServiceExt;

        use crate::core::common::api::compose_admin_routes;
        use crate::core::common::local_auth::LocalAuthConfig;
        use crate::core::common::unified_auth::UnifiedAuthState;

        let dir = std::env::temp_dir().join("edgion_web_test_authorder");
        let _ = std::fs::create_dir_all(dir.join("assets"));
        std::fs::write(dir.join("index.html"), b"<html>shell</html>").unwrap();
        std::fs::write(dir.join("assets/app.js"), b"console.log(1)").unwrap();
        let source = Arc::new(WebSource::Dir(dir.clone()));

        let local = LocalAuthConfig {
            username: "admin".to_string(),
            password: "a_long_enough_password_123".to_string(),
            jwt_secret: "a_long_enough_jwt_secret_value_abcdef".to_string(),
            ..LocalAuthConfig::default()
        };
        let state = UnifiedAuthState::from_configs(None, Some(&local), true, "test").unwrap();

        let business = Router::new().route("/api/v1/secret", get(|| async { "secret" }));
        let app = compose_admin_routes(business, state, true);
        let app = app.fallback(move |uri: Uri| {
            let source = source.clone();
            async move { serve(source, uri).await }
        });

        // Protected API route: 401 without credentials.
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/api/v1/secret").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "API route must stay protected");

        // Public asset: 200 with no credentials.
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/assets/app.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "asset must be public");

        // SPA client route: 200 shell with no credentials.
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/routes").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "SPA route must serve the shell");

        // Unknown API path: 404, never the SPA shell.
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/api/v1/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "unknown API path must 404");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exact_asset_is_served_with_immutable_cache() {
        let dir = std::env::temp_dir().join("edgion_web_test_asset");
        let _ = std::fs::create_dir_all(dir.join("assets"));
        std::fs::write(dir.join("index.html"), b"shell").unwrap();
        std::fs::write(dir.join("assets/app-abc.js"), b"console.log(1)").unwrap();
        let src = Arc::new(WebSource::Dir(dir.clone()));

        let resp = serve(src.clone(), "/assets/app-abc.js".parse().unwrap()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/javascript; charset=utf-8"
        );
        assert!(resp
            .headers()
            .get(header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("immutable"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
