use super::pick_effective_sp;
use edgion_resources::resources::http_route::SessionPersistence;

fn sp(name: &str) -> SessionPersistence {
    SessionPersistence {
        session_name: Some(name.to_string()),
        absolute_timeout: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    }
}

#[test]
fn http_only_uses_http() {
    let http = sp("http-sp");
    let picked = pick_effective_sp(Some(&http), None).expect("http-only must resolve");
    assert_eq!(picked.session_name.as_deref(), Some("http-sp"));
}

#[test]
fn grpc_only_falls_back_to_grpc() {
    let grpc = sp("grpc-sp");
    let picked = pick_effective_sp(None, Some(&grpc)).expect("grpc-only must resolve");
    assert_eq!(picked.session_name.as_deref(), Some("grpc-sp"));
}

#[test]
fn neither_yields_none() {
    assert!(pick_effective_sp(None, None).is_none());
}

#[test]
fn http_wins_over_grpc() {
    // Only one route unit is ever set per request; this pins the tie-break
    // order for consistency with retry_attempts()/retry_backoff().
    let http = sp("http-sp");
    let grpc = sp("grpc-sp");
    let picked = pick_effective_sp(Some(&http), Some(&grpc)).expect("must resolve");
    assert_eq!(picked.session_name.as_deref(), Some("http-sp"));
}
