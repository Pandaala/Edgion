use super::build_token_auth_status;
use crate::authz::{sha256_hex, Authorizer, CenterPolicy, CliTokenEntry};
use edgion_resources::auth_dto::AuthStatus;
use std::sync::Arc;

/// No token registered → Initializing, auth always required, mode "token".
#[test]
fn status_initializing_without_token() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    let resp = build_token_auth_status(&az);
    assert!(resp.auth_required);
    assert_eq!(resp.mode, "token");
    assert_eq!(resp.status, AuthStatus::Initializing);
}

/// At least one token registered → Ready.
#[test]
fn status_ready_with_token() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    let mut m = std::collections::HashMap::new();
    m.insert(
        sha256_hex("tok"),
        CliTokenEntry {
            name: "bot".into(),
            policy: Arc::new(CenterPolicy::deny_all()),
        },
    );
    az.apply(CenterPolicy::deny_all(), m);
    let resp = build_token_auth_status(&az);
    assert!(resp.auth_required);
    assert_eq!(resp.status, AuthStatus::Ready);
}

/// The local admin token alone makes status Ready (it is merged by `apply`).
#[test]
fn status_ready_with_admin_token_only() {
    let az = Authorizer::new(CenterPolicy::deny_all());
    az.set_admin_token(crate::authz::build_admin_token_entry(Some("admin-secret")));
    az.apply(CenterPolicy::deny_all(), std::collections::HashMap::new());
    assert_eq!(build_token_auth_status(&az).status, AuthStatus::Ready);
}
