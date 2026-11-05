use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use crate::plugins::runtime::PluginLog;
use base64::engine::general_purpose;
use serde_json::Value;
use std::sync::Arc;

#[test]
fn consumer_identity_header_preserves_bytes_and_marks_sensitive() {
    let auth = BasicAuth::new(&BasicAuthConfig::default());
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Consumer-Username" && value == "private-user" && value.is_sensitive())
        .times(1)
        .returning(|_, _| Ok(()));
    auth.set_consumer_headers(&mut session, "private-user").unwrap();
    assert!(auth.set_consumer_headers(&mut session, "invalid\r\nidentity").is_err());
}

fn make_user_map(username: &str, password: &str, group: &str) -> HashMap<String, ResolvedBasicAuthUser> {
    let mut map = HashMap::new();
    map.insert(
        username.to_string(),
        ResolvedBasicAuthUser {
            password: password.to_string(),
            group: group.to_string(),
        },
    );
    map
}

fn create_basic_auth_with_users() -> BasicAuth {
    let config = BasicAuthConfig {
        realm: "Test Realm".to_string(),
        resolved_users: Some(make_user_map("testuser", "testpass", "default")),
        ..Default::default()
    };
    BasicAuth::new(&config)
}

fn basic_header(decoded: &[u8]) -> String {
    format!("Basic {}", general_purpose::STANDARD.encode(decoded))
}

fn prepared_users(username: &str, password: &str, group: &str) -> PreparedUserMap {
    BasicAuth::prepare_users(make_user_map(username, password, group))
}

#[test]
fn test_extract_credentials_case_insensitive_scheme() {
    let header = format!(
        "basic {}",
        general_purpose::STANDARD.encode("testuser:testpass".as_bytes())
    );
    let (username, password) = BasicAuth::extract_credentials(&header).expect("should parse");
    assert_eq!(username, "testuser");
    assert_eq!(password, "testpass");
}

#[test]
fn test_extract_credentials_preserves_spaces_and_password_colons() {
    let header = basic_header(b" user : pass :tail");
    let (username, password) = BasicAuth::extract_credentials(&header).expect("should parse");
    assert_eq!(username, " user ");
    assert_eq!(password, " pass :tail");
}

#[test]
fn test_extract_credentials_error_matrix() {
    let cases = [
        ("Basic".to_string(), "Invalid authorization header format"),
        ("Bearer abc".to_string(), "Invalid authorization header format"),
        ("Basic   ".to_string(), "Invalid authorization header format"),
        ("Basic !!!".to_string(), "Failed to decode authentication header"),
        (basic_header(&[0xff]), "Invalid UTF-8 in decoded authorization"),
        (basic_header(b"username"), "Invalid decoded data format"),
        (basic_header(b":password"), "Empty username or password"),
        (basic_header(b"username:"), "Empty username or password"),
    ];

    for (header, expected) in cases {
        let error = BasicAuth::extract_credentials(&header).expect_err("input must be rejected");
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn password_prefix_classification_is_exact_and_case_sensitive() {
    for password in ["{SHA}hash", "$apr1$salt$hash", "$1$salt$hash"] {
        assert!(matches!(
            BasicAuth::classify_password(password),
            PasswordEncoding::SupportedHash
        ));
    }
    for password in [
        "$2$hash",
        "$2a$hash",
        "$2b$hash",
        "$2x$hash",
        "$2y$hash",
        "$5$salt$hash",
        "$6$salt$hash",
    ] {
        assert!(matches!(
            BasicAuth::classify_password(password),
            PasswordEncoding::RecognizedUnsupportedHash
        ));
    }
    for password in ["{sha}hash", "$APR1$salt$hash", "$2A$hash", "plain"] {
        assert!(matches!(
            BasicAuth::classify_password(password),
            PasswordEncoding::Plaintext
        ));
    }
}

#[test]
fn prepare_users_moves_supported_and_unsupported_buffers() {
    let supported_password = "{SHA}W6ph5Mm5Pz8GgiULbPgzG37mj9g=".to_string();
    let supported_password_ptr = supported_password.as_ptr();
    let supported_group = "supported-group".to_string();
    let supported_group_ptr = supported_group.as_ptr();
    let unsupported_password = "$2b$12$recognized-but-unsupported".to_string();
    let unsupported_password_ptr = unsupported_password.as_ptr();
    let unsupported_group = "unsupported-group".to_string();
    let unsupported_group_ptr = unsupported_group.as_ptr();
    let users = HashMap::from([
        (
            "supported".to_string(),
            ResolvedBasicAuthUser {
                password: supported_password,
                group: supported_group,
            },
        ),
        (
            "unsupported".to_string(),
            ResolvedBasicAuthUser {
                password: unsupported_password,
                group: unsupported_group,
            },
        ),
    ]);

    let prepared = BasicAuth::prepare_users(users);
    let supported = prepared.get("supported").unwrap();
    assert_eq!(supported.password_hash.as_ptr(), supported_password_ptr);
    assert_eq!(supported.group.as_ptr(), supported_group_ptr);
    let unsupported = prepared.get("unsupported").unwrap();
    assert_eq!(unsupported.password_hash.as_ptr(), unsupported_password_ptr);
    assert_eq!(unsupported.group.as_ptr(), unsupported_group_ptr);
}

#[test]
fn recognized_unsupported_hashes_remain_unchanged_and_reject_hash_literal() {
    for password_hash in [
        "$2$hash",
        "$2a$hash",
        "$2b$hash",
        "$2x$hash",
        "$2y$hash",
        "$5$salt$hash",
        "$6$salt$hash",
    ] {
        let users = prepared_users("alice", password_hash, "partner-a");
        assert_eq!(users.get("alice").unwrap().password_hash, password_hash);
        assert!(BasicAuth::verify_user(&users, "alice", password_hash).is_none());
    }
}

#[test]
fn plaintext_is_prepared_as_sha_and_verifies() {
    let users = prepared_users("alice", "password", "partner-a");
    assert!(users.get("alice").unwrap().password_hash.starts_with("{SHA}"));
    assert_eq!(BasicAuth::verify_user(&users, "alice", "password"), Some("partner-a"));
    assert!(BasicAuth::verify_user(&users, "alice", "wrong").is_none());
    assert!(BasicAuth::verify_user(&users, "bob", "password").is_none());
}

#[test]
fn constructor_removes_resolved_users_from_retained_config() {
    let config = BasicAuthConfig {
        resolved_users: Some(make_user_map("alice", "password", "partner-a")),
        ..Default::default()
    };

    let plugin = BasicAuth::new(&config);

    assert!(plugin.config.resolved_users.is_none());
    assert!(config.resolved_users.as_ref().unwrap().contains_key("alice"));
    assert!(plugin.user_passwords.contains_key("alice"));
}

#[test]
fn each_prepared_generation_verifies_only_its_own_user_version() {
    let old = prepared_users("alice", "old-password", "old-group");
    let new = prepared_users("alice", "new-password", "new-group");

    assert_eq!(BasicAuth::verify_user(&old, "alice", "old-password"), Some("old-group"));
    assert!(BasicAuth::verify_user(&old, "alice", "new-password").is_none());
    assert_eq!(BasicAuth::verify_user(&new, "alice", "new-password"), Some("new-group"));
    assert!(BasicAuth::verify_user(&new, "alice", "old-password").is_none());
}

#[test]
fn apr1_and_md5_crypt_hashes_verify_inline() {
    // `openssl passwd -apr1 -salt r31.....` / `openssl passwd -1 -salt saltsalt` of "myPassword".
    for hash in [
        "$apr1$r31.....$HqJZimcKQFAMYayBlzkrA/",
        "$1$saltsalt$2vnaRpHa6Jxjz5n83ok8Z0",
    ] {
        let users = prepared_users("alice", hash, "partner-a");
        assert_eq!(BasicAuth::verify_user(&users, "alice", "myPassword"), Some("partner-a"));
        assert!(BasicAuth::verify_user(&users, "alice", "wrong").is_none());
    }
}

// A plain #[test] with no Tokio runtime: authentication must stay synchronous
// (no blocking-pool dispatch) for this to compile and run.
#[test]
fn known_and_unknown_users_authenticate_synchronously() {
    let plugin = create_basic_auth_with_users();

    let mut known = MockPluginSession::new();
    setup_header_mocks(
        &mut known,
        &[("authorization", basic_header(b"testuser:testpass").as_str())],
    );
    assert_eq!(
        plugin.authenticate_request(&mut known).unwrap(),
        ("testuser".to_string(), "default")
    );

    let mut unknown = MockPluginSession::new();
    setup_header_mocks(
        &mut unknown,
        &[("authorization", basic_header(b"missing:testpass").as_str())],
    );
    assert!(matches!(
        plugin.authenticate_request(&mut unknown),
        Err(AuthFailure::BadCredentials)
    ));
}

fn render_log(log: &PluginLog) -> Vec<String> {
    let v: Value = serde_json::to_value(log).unwrap();
    v.get("log")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

#[test]
fn basic_hit_log_has_exact_group_and_user_shape() {
    let mut log = PluginLog::new("BasicAuth");
    let group = "partner-a";
    let username = "alice";
    log.push_fmt("OK group={} user {}", la![group, sensitive(username)]);

    let entries = render_log(&log);
    assert_eq!(entries, vec!["OK group=partner-a user alice"]);
}

#[test]
fn basic_failure_logs_have_exact_non_attributing_shape() {
    for expected in ["Allow anonymous", "Deny no credential", "Deny bad credential"] {
        let mut log = PluginLog::new("BasicAuth");
        log.push(expected);
        let entries = render_log(&log);
        assert_eq!(entries, vec![expected]);
        assert!(!entries[0].contains("group="));
        assert!(!entries[0].contains("user "));
    }
}

#[tokio::test]
async fn prepared_challenge_rejects_invalid_direct_construction_before_writes() {
    let auth = BasicAuth::new(&BasicAuthConfig {
        realm: "private\r\nrealm".to_string(),
        ..Default::default()
    });
    let mut session = MockPluginSession::new();
    session.expect_write_response_header().never();
    session.expect_write_response_body().never();
    session.expect_shutdown().never();
    assert!(auth.auth_failed_return(&mut session).await.is_err());
}

#[tokio::test]
async fn prepared_challenge_tracks_plugin_generations_and_releases_old_owner() {
    let old = Arc::new(BasicAuth::new(&BasicAuthConfig {
        realm: "old".to_string(),
        ..Default::default()
    }));
    let old_weak = Arc::downgrade(&old);
    let inflight = old.clone();
    let current = Arc::new(BasicAuth::new(&BasicAuthConfig {
        realm: "new \"API\"\\".to_string(),
        ..Default::default()
    }));
    drop(old);
    for (owner, expected) in [
        (&inflight, "Basic realm=\"old\""),
        (&current, "Basic realm=\"new \\\"API\\\"\\\\\""),
    ] {
        let mut session = MockPluginSession::new();
        session
            .expect_write_response_header()
            .times(1)
            .returning(move |header, _| {
                assert_eq!(header.headers["www-authenticate"].as_bytes(), expected.as_bytes());
                Ok(())
            });
        session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
        session.expect_shutdown().times(1).returning(|| {});
        owner.auth_failed_return(&mut session).await.unwrap();
    }
    assert!(old_weak.upgrade().is_some());
    drop(inflight);
    assert!(old_weak.upgrade().is_none());
}
