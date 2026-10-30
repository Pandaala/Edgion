use super::*;
use crate::conf_mgr::conf_center::traits::LeadershipStatus;
use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};
use tower::ServiceExt;

#[test]
fn follower_rejects_business_paths_with_leader_addr() {
    let status = LeadershipStatus::Follower {
        leader_admin_addr: Some("10.0.0.11:8080".to_string()),
    };
    for path in [
        "/api/v1/cluster/EdgionTls",
        "/api/v1/namespaced/HTTPRoute/default",
        "/api/v1/server-info",
        "/configserver/HTTPRoute/list",
    ] {
        let (code, body) = not_leader_rejection(&status, path).expect("business path must be rejected");
        assert_eq!(code, StatusCode::CONFLICT, "{path}");
        assert_eq!(body["error"], "not leader");
        assert_eq!(body["leader"], "10.0.0.11:8080");
    }
}

#[test]
fn follower_without_known_leader_rejects_with_null_leader() {
    let status = LeadershipStatus::Follower {
        leader_admin_addr: None,
    };
    let (code, body) = not_leader_rejection(&status, "/api/v1/server-info").expect("rejected");
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(body["leader"].is_null(), "unknown leader must serialize as null");
}

#[test]
fn follower_keeps_public_paths_open() {
    let status = LeadershipStatus::Follower {
        leader_admin_addr: Some("10.0.0.11:8080".to_string()),
    };
    for path in ["/health", "/ready", "/metrics", "/api/v1/auth/status"] {
        assert!(
            not_leader_rejection(&status, path).is_none(),
            "{path} must stay open on a standby"
        );
    }
}

#[test]
fn leader_and_not_applicable_pass_everything() {
    for status in [LeadershipStatus::Leader, LeadershipStatus::NotApplicable] {
        assert!(not_leader_rejection(&status, "/api/v1/server-info").is_none());
        assert!(not_leader_rejection(&status, "/api/v1/cluster/EdgionTls").is_none());
    }
}

/// S3/S4 zero-impact guard: a FileSystem-backed ConfMgr (leadership
/// `NotApplicable`) must never see a 409 from the gate on the full router.
#[tokio::test]
async fn gate_is_passthrough_when_not_applicable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(
            FileSystemConfig::new(tmp.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller"),
        ))
        .await
        .expect("ConfMgr::create"),
    );
    assert_eq!(conf_mgr.leadership_status(), LeadershipStatus::NotApplicable);

    let router = create_admin_router(conf_mgr, Arc::new(SchemaValidator::empty()));

    let removed = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/reload")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NOT_FOUND);

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/server-info")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::CONFLICT,
        "NotApplicable must disable the not-leader gate"
    );
}
