//! Admin API IP allowlist (L7 enforcement, fr admin-api-02).
//!
//! Rejects requests whose TCP peer IP is not in the operator-configured CIDR
//! allowlist, BEFORE authentication. The decision input is the kernel TCP peer
//! (`ConnectInfo<SocketAddr>`) — never X-Forwarded-For, which is spoofable.
//!
//! - Empty/unset allowlist ⇒ the layer is not mounted (allow all; see callers).
//! - Missing `ConnectInfo` ⇒ 403 (fail-closed): guards a serve entry that forgot
//!   `into_make_service_with_connect_info`.
//! - IPv4-mapped IPv6 peers (`::ffff:a.b.c.d` on a `::`-bound listener) are
//!   unmapped via `IpAddr::to_canonical()` before matching.
//!
//! Reuses the existing `ip_cidr_helpers` / `ip_radix_tree` matcher — no new crate.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use edgion_resources::matcher::ip_cidr_helpers::{to_cidr_format, validate_ip_or_cidr};
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;

/// Single group label for the flat admin allowlist matcher.
const ADMIN_ALLOW_GROUP: &str = "admin-allow";

/// Validate every CIDR entry (fail-fast at config load). Reuses
/// `validate_ip_or_cidr`; does not build a matcher.
pub fn validate_admin_ips(cidrs: &[String]) -> anyhow::Result<()> {
    for cidr in cidrs {
        validate_ip_or_cidr(cidr).map_err(|e| anyhow::anyhow!("invalid allow_admin_ips entry: {}", e))?;
    }
    Ok(())
}

/// Build the allowlist matcher. `Ok(None)` when empty (allow-all: caller must not
/// mount the layer). `Err` on a malformed CIDR (defense in depth; load validates).
pub fn build_admin_ip_matcher(cidrs: &[String]) -> anyhow::Result<Option<Arc<IpRadixMatcher>>> {
    if cidrs.is_empty() {
        return Ok(None);
    }
    let mut builder = IpRadixMatcher::builder();
    for cidr in cidrs {
        let norm =
            to_cidr_format(cidr).map_err(|e| anyhow::anyhow!("invalid allow_admin_ips entry '{}': {}", cidr, e))?;
        builder
            .insert(&norm, ADMIN_ALLOW_GROUP)
            .map_err(|e| anyhow::anyhow!("invalid allow_admin_ips entry '{}': {}", cidr, e))?;
    }
    let matcher = builder
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build admin IP allowlist matcher: {}", e))?;
    Ok(Some(Arc::new(matcher)))
}

/// Exact 403 body shape (field order guaranteed by struct declaration order).
#[derive(serde::Serialize)]
struct DeniedBody {
    success: bool,
    error: &'static str,
}

fn denied_response() -> Response {
    let mut resp = (
        StatusCode::FORBIDDEN,
        axum::Json(DeniedBody {
            success: false,
            error: "Access denied",
        }),
    )
        .into_response();
    // The IP layer sits OUTSIDE compose's cache-control middleware, so set it here
    // to keep the `Cache-Control: no-store` contract on every admin response.
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Rate-limited denial log: 1st, then every 256th, to bound volume under a flood.
fn log_denied(peer: Option<std::net::IpAddr>) {
    static DENIED: AtomicU64 = AtomicU64::new(0);
    let n = DENIED.fetch_add(1, Ordering::Relaxed);
    if n == 0 || n % 256 == 0 {
        tracing::warn!(
            component = "admin_ip_allowlist",
            peer = ?peer,
            denied_total = n + 1,
            "admin API request denied by IP allowlist"
        );
    }
}

/// Middleware: pass through when the (canonicalized) TCP peer matches the
/// allowlist; otherwise 403. Fail-closed when `ConnectInfo` is absent.
pub async fn ip_allowlist_middleware(
    State(matcher): State<Arc<IpRadixMatcher>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_canonical());
    match peer {
        Some(ip) if matcher.matched_group(&ip).is_some() => next.run(req).await,
        other => {
            log_denied(other);
            denied_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn matcher(cidrs: &[&str]) -> Arc<IpRadixMatcher> {
        let v: Vec<String> = cidrs.iter().map(|s| s.to_string()).collect();
        build_admin_ip_matcher(&v).unwrap().unwrap()
    }

    fn app(m: Arc<IpRadixMatcher>) -> Router {
        Router::new()
            .route("/foo", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(m, ip_allowlist_middleware))
    }

    async fn run(app: Router, peer: Option<&str>) -> axum::http::Response<Body> {
        let mut req = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        if let Some(p) = peer {
            let addr: SocketAddr = p.parse().unwrap();
            req.extensions_mut().insert(ConnectInfo(addr));
        }
        app.oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn ipv4_in_cidr_allowed() {
        let resp = run(app(matcher(&["10.0.0.0/8"])), Some("10.1.2.3:5000")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ipv4_outside_cidr_denied() {
        let resp = run(app(matcher(&["10.0.0.0/8"])), Some("192.168.1.1:5000")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ipv6_in_cidr_allowed() {
        let resp = run(app(matcher(&["2001:db8::/32"])), Some("[2001:db8::1]:5000")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Key regression: a dual-stack listener reports an IPv4 client as
    /// `::ffff:10.1.2.3`; it must match an IPv4 CIDR after to_canonical().
    #[tokio::test]
    async fn ipv4_mapped_ipv6_matches_ipv4_cidr() {
        let resp = run(app(matcher(&["10.0.0.0/8"])), Some("[::ffff:10.1.2.3]:5000")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Fail-closed: no ConnectInfo ⇒ 403.
    #[tokio::test]
    async fn missing_connect_info_denied() {
        let resp = run(app(matcher(&["10.0.0.0/8"])), None).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn denied_response_shape_and_headers() {
        let resp = run(app(matcher(&["10.0.0.0/8"])), Some("192.168.1.1:5000")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).map(|v| v.as_bytes()),
            Some(b"no-store".as_slice())
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], br#"{"success":false,"error":"Access denied"}"#);
    }

    #[test]
    fn empty_allowlist_returns_none() {
        assert!(build_admin_ip_matcher(&[]).unwrap().is_none());
    }

    #[test]
    fn malformed_cidr_errors() {
        assert!(build_admin_ip_matcher(&["10.0.0/8".to_string()]).is_err());
        assert!(validate_admin_ips(&["10.0.0/8".to_string()]).is_err());
        assert!(validate_admin_ips(&["10.0.0.0/8".to_string(), "::1".to_string()]).is_ok());
    }
}
