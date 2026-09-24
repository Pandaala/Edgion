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
