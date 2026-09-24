use super::*;
use crate::authz::{Authorizer, CenterPolicy, Verb};
use std::io::Write;
use std::sync::Arc;

/// Build a minimal AdminState for reload tests:
/// - config_file points to a caller-supplied temp path
/// - Authorizer starts deny-all
/// - watch channel created fresh
///
/// `apply_center_reload` only touches `config_file`, `authorizer`, and
/// `center_reload_tx`; the ConfMgr is real but not exercised.
async fn make_reload_state(
    config_path: &str,
) -> (
    Arc<AdminState>,
    tokio::sync::watch::Receiver<crate::config::CenterClientConfig>,
    tempfile::TempDir, // kept alive so its path stays valid
) {
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
    let fs_config =
        FileSystemConfig::new(tmp_data.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
            .await
            .expect("ConfMgr::create"),
    );
    let schema_validator = Arc::new(SchemaValidator::empty());
    let (tx, rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
    let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
    let state = Arc::new(AdminState {
        conf_mgr,
        schema_validator,
        config_file: Some(config_path.to_string()),
        authorizer,
        center_reload_tx: tx,
    });
    (state, rx, tmp_data)
}

/// Write a minimal controller config YAML that includes a [center] block
/// with an RBAC rule granting `get` on `EdgionConfigData`.
fn write_center_config(path: &str) {
    let yaml = r#"
center:
  address: "http://center.example.com:50052"
  name: "ctrl-test"
  cluster: "test-cluster"
  enabled: true
  rbac:
    allow:
      - verbs: [get, list]
        kinds: [EdgionConfigData]
"#;
    let mut f = std::fs::File::create(path).expect("create config file");
    f.write_all(yaml.as_bytes()).expect("write config");
}

#[tokio::test]
async fn reload_rereads_center_and_swaps_policy_before_request_reload() {
    let tmp_cfg = tempfile::NamedTempFile::new().expect("named tempfile");
    let cfg_path = tmp_cfg.path().to_str().expect("path str").to_string();

    write_center_config(&cfg_path);

    let (state, mut rx, _tmp_data) = make_reload_state(&cfg_path).await;

    // Before: authorizer is deny-all.
    assert!(
        !state
            .authorizer
            .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
            .allowed(),
        "pre-condition: deny-all should deny"
    );

    // Mark the current value as seen so `has_changed()` is meaningful.
    rx.borrow_and_update();

    // Run the reload helper (the part that doesn't need ConfMgr to be live).
    apply_center_reload(&state);

    // Assertion 1: policy is now swapped -- Center can GET EdgionConfigData.
    assert!(
        state
            .authorizer
            .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
            .allowed(),
        "post: authorizer must allow get/EdgionConfigData after policy swap"
    );
    // Also ensure deny-all still applies for ungranted pairs.
    assert!(
        !state
            .authorizer
            .authorize(&state.authorizer.center_role(), Verb::Delete, "EdgionConfigData")
            .allowed(),
        "post: delete/EdgionConfigData must remain denied"
    );

    // Assertion 2: center_reload_tx has a new value (supervisor notified).
    assert!(
        rx.has_changed().expect("channel not closed"),
        "supervisor channel must have a new value"
    );
    let new_cfg = rx.borrow_and_update();
    assert_eq!(new_cfg.name, "ctrl-test", "center config name must match YAML");
    assert!(new_cfg.rbac.is_some(), "center rbac must be present in new value");
}

#[tokio::test]
async fn reload_missing_config_file_keeps_current_policy() {
    // If the config file does not exist, warn and keep current policy.
    let (state, mut rx, _tmp_data) = make_reload_state("/nonexistent/path/edgion-controller.yaml").await;
    rx.borrow_and_update();

    apply_center_reload(&state);

    // Policy must remain deny-all.
    assert!(
        !state
            .authorizer
            .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
            .allowed(),
        "policy must remain deny-all when config file is missing"
    );
    // Channel must NOT have received a new value.
    assert!(
        !rx.has_changed().expect("channel not closed"),
        "supervisor channel must NOT be notified when config load fails"
    );
}

#[tokio::test]
async fn reload_no_center_section_does_not_notify() {
    // Config file with no [center] block: policy unchanged, no notify.
    let tmp_cfg = tempfile::NamedTempFile::new().expect("named tempfile");
    let cfg_path = tmp_cfg.path().to_str().expect("path str").to_string();

    // Write a valid config with no center section.
    std::fs::write(&cfg_path, "# no center section\n").expect("write");

    let (state, mut rx, _tmp_data) = make_reload_state(&cfg_path).await;
    rx.borrow_and_update();

    apply_center_reload(&state);

    assert!(
        !state
            .authorizer
            .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
            .allowed(),
        "policy must remain deny-all when no [center] section present"
    );
    assert!(
        !rx.has_changed().expect("channel not closed"),
        "supervisor channel must NOT be notified when [center] is absent"
    );
}

#[tokio::test]
async fn reload_no_config_file_is_noop() {
    // When config_file is None, apply_center_reload returns immediately --
    // the authorizer policy stays deny-all and the channel is not notified.
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
    let fs_config =
        FileSystemConfig::new(tmp_data.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
            .await
            .expect("ConfMgr::create"),
    );
    let schema_validator = Arc::new(SchemaValidator::empty());
    let (tx, mut rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
    let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
    let state = AdminState {
        conf_mgr,
        schema_validator,
        config_file: None, // no config file
        authorizer,
        center_reload_tx: tx,
    };

    rx.borrow_and_update();

    apply_center_reload(&state);

    // Policy must remain deny-all.
    assert!(
        !state
            .authorizer
            .authorize(&state.authorizer.center_role(), Verb::Get, "EdgionConfigData")
            .allowed(),
        "policy must be unchanged (deny-all) when config_file is None"
    );
    // Channel must NOT have received a new value.
    assert!(
        !rx.has_changed().expect("channel not closed"),
        "supervisor channel must NOT be notified when config_file is None"
    );
}

/// Reload that adds a cli_token makes `authorizer.cli_lookup(tok)` resolve afterward.
///
/// Also verifies that cli_tokens are hot-reloaded independently of `[center]`:
/// the config written here has a cli_tokens entry but NO [center] section.
#[tokio::test]
async fn reload_cli_token_added_resolves_after_reload() {
    use crate::authz::sha256_hex;

    let tmp_cfg = tempfile::NamedTempFile::new().expect("named tempfile");
    let cfg_path = tmp_cfg.path().to_str().expect("path str").to_string();

    // Raw token value (not pre-hashed); the config stores sha256_hex(tok).
    let raw_token = "test-cli-token-abc123";
    let token_hash = sha256_hex(raw_token);

    let yaml = format!(
        r#"
cli_tokens:
  - name: reload-bot
    token_sha256: "{}"
    rbac:
      allow:
        - verbs: [get, list]
          kinds: [EdgionConfigData]
"#,
        token_hash
    );
    std::fs::write(&cfg_path, yaml.as_bytes()).expect("write config");

    let (state, mut rx, _tmp_data) = make_reload_state(&cfg_path).await;
    rx.borrow_and_update();

    // Before reload: no cli tokens registered.
    assert!(
        state.authorizer.cli_lookup(raw_token).is_none(),
        "before reload: no cli token"
    );
    assert!(
        !state.authorizer.cli_tokens_present(),
        "before reload: cli_tokens_present must be false"
    );

    apply_center_reload(&state);

    // After reload: cli_lookup resolves and role is correct.
    let (role, name) = state
        .authorizer
        .cli_lookup(raw_token)
        .expect("after reload: cli_lookup must hit");
    assert_eq!(name, "reload-bot");
    assert!(
        state
            .authorizer
            .authorize(&role, Verb::Get, "EdgionConfigData")
            .allowed(),
        "after reload: get/EdgionConfigData must be allowed"
    );
    assert!(
        !state
            .authorizer
            .authorize(&role, Verb::Delete, "EdgionConfigData")
            .allowed(),
        "after reload: delete must remain denied"
    );
    assert!(
        state.authorizer.cli_tokens_present(),
        "after reload: cli_tokens_present must be true"
    );

    // No [center] section => supervisor channel must NOT be notified.
    assert!(
        !rx.has_changed().expect("channel not closed"),
        "supervisor must NOT be notified when no [center] section is present"
    );
}

/// Startup drop-invalid: a cli_token with a bad token_sha256 is dropped,
/// valid entries are still loaded.
#[tokio::test]
async fn reload_cli_token_drop_invalid_keeps_valid() {
    use crate::authz::sha256_hex;

    let tmp_cfg = tempfile::NamedTempFile::new().expect("named tempfile");
    let cfg_path = tmp_cfg.path().to_str().expect("path str").to_string();

    let valid_raw = "valid-token-xyz";
    let valid_hash = sha256_hex(valid_raw);

    // One valid entry + one invalid entry (bad hash).
    let yaml = format!(
        r#"
cli_tokens:
  - name: valid-bot
    token_sha256: "{}"
    rbac:
      allow:
        - verbs: [get]
          kinds: [EdgionConfigData]
  - name: bad-bot
    token_sha256: "notahexhash"
    rbac:
      allow:
        - verbs: [get]
          kinds: [EdgionConfigData]
"#,
        valid_hash
    );
    std::fs::write(&cfg_path, yaml.as_bytes()).expect("write config");

    let (state, mut rx, _tmp_data) = make_reload_state(&cfg_path).await;
    rx.borrow_and_update();

    apply_center_reload(&state);

    // valid-bot must resolve; bad-bot must be dropped.
    assert!(
        state.authorizer.cli_lookup(valid_raw).is_some(),
        "valid-bot must resolve after reload"
    );
    assert!(
        state.authorizer.cli_lookup("notahexhash").is_none(),
        "bad-bot must be dropped"
    );
    assert!(
        state.authorizer.cli_tokens_present(),
        "at least one valid entry => present"
    );

    assert!(
        !rx.has_changed().expect("channel not closed"),
        "no [center] section => supervisor not notified"
    );
}
