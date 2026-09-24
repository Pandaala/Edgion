use super::update_identity_tests::make_fs_state;
use super::*;

/// `config.denys` is a typo for `config.deny` on `IpRestrictionConfig`.
fn plugins_body_with_typo(name: &str, namespace: &str) -> Bytes {
    Bytes::from(format!(
        r#"{{"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{{"name":"{name}","namespace":"{namespace}"}},"spec":{{"requestPlugins":[{{"type":"IpRestriction","config":{{"defaultAction":"allow","denys":[]}}}}]}}}}"#
    ))
}

fn plugins_body_clean(name: &str, namespace: &str) -> Bytes {
    Bytes::from(format!(
        r#"{{"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{{"name":"{name}","namespace":"{namespace}"}},"spec":{{"requestPlugins":[{{"type":"IpRestriction","config":{{"defaultAction":"allow","deny":[]}}}}]}}}}"#
    ))
}

fn config_data_body(name: &str, typo: bool) -> Bytes {
    let typo_field = if typo { r#","cidrz":[]"# } else { "" };
    Bytes::from(format!(
        r#"{{"apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{{"name":"{name}","namespace":"default"}},"spec":{{"data":{{"type":"IpList","config":{{"items":[{{"name":"office","cidrs":[]{typo_field}}}]}}}}}}}}"#
    ))
}

fn link_sys_body(name: &str, typo: bool) -> Bytes {
    let typo_field = if typo { r#", "conect": 20"# } else { "" };
    Bytes::from(
            r#"{"apiVersion":"edgion.io/v1","kind":"LinkSys","metadata":{"name":"NAME","namespace":"default"},"spec":{"type":"redis","config":{"endpoints":["redis://127.0.0.1:6379"],"timeout":{"connect":"3s"TYPO}}}}"#
                .replace("NAME", name)
                .replace("TYPO", typo_field),
        )
}

#[tokio::test]
async fn create_with_typo_is_not_rejected_and_carries_warning() {
    let (state, _tmp) = make_fs_state().await;
    let result = create_namespaced(
        State(state),
        Path(("EdgionPlugins".to_string(), "default".to_string())),
        plugins_body_with_typo("auth-chain", "default"),
    )
    .await;

    let (status, _headers, body) = result.expect("a typo'd key must warn, never reject the write");
    assert_eq!(
        status,
        StatusCode::CREATED,
        "warn-not-reject: the write must still succeed"
    );
    assert_eq!(body.0.warnings, vec!["requestPlugins[0].config.denys".to_string()]);

    let serialized = serde_json::to_value(&body.0).expect("ApiResponse must serialize");
    assert_eq!(
        serialized["warnings"][0], "requestPlugins[0].config.denys",
        "the finding must also be visible in the serialized payload"
    );
}

#[tokio::test]
async fn create_with_clean_document_has_no_warnings_key_in_serialized_payload() {
    let (state, _tmp) = make_fs_state().await;
    let result = create_namespaced(
        State(state),
        Path(("EdgionPlugins".to_string(), "default".to_string())),
        plugins_body_clean("clean-chain", "default"),
    )
    .await;

    let (status, _headers, body) = result.expect("a clean document must create successfully");
    assert_eq!(status, StatusCode::CREATED);
    assert!(body.0.warnings.is_empty());

    let serialized = serde_json::to_value(&body.0).expect("ApiResponse must serialize");
    assert!(
        serialized.get("warnings").is_none(),
        "empty warnings must be entirely absent from the JSON payload, not an empty array"
    );
}

#[tokio::test]
async fn update_with_typo_is_not_rejected_and_carries_warning() {
    let (state, _tmp) = make_fs_state().await;
    let _ = create_namespaced(
        State(Arc::clone(&state)),
        Path(("EdgionPlugins".to_string(), "default".to_string())),
        plugins_body_clean("auth-chain", "default"),
    )
    .await
    .expect("seed create must succeed");

    let result = update_namespaced(
        State(state),
        Path((
            "EdgionPlugins".to_string(),
            "default".to_string(),
            "auth-chain".to_string(),
        )),
        HeaderMap::new(),
        plugins_body_with_typo("auth-chain", "default"),
    )
    .await;

    let body = result.expect("a typo'd key must warn, never reject the update");
    assert_eq!(body.0.warnings, vec!["requestPlugins[0].config.denys".to_string()]);
}

#[tokio::test]
async fn config_data_create_and_update_carry_typed_unknown_field_warning() {
    let (state, _tmp) = make_fs_state().await;
    let created = create_namespaced(
        State(Arc::clone(&state)),
        Path(("EdgionConfigData".to_string(), "default".to_string())),
        config_data_body("cidrs", true),
    )
    .await
    .expect("advisory finding must not reject create");
    assert_eq!(created.2 .0.warnings, ["data.config.items[0].cidrz"]);

    let updated = update_namespaced(
        State(state),
        Path((
            "EdgionConfigData".to_string(),
            "default".to_string(),
            "cidrs".to_string(),
        )),
        HeaderMap::new(),
        config_data_body("cidrs", true),
    )
    .await
    .expect("advisory finding must not reject update");
    assert_eq!(updated.0.warnings, ["data.config.items[0].cidrz"]);
}

#[tokio::test]
async fn config_data_clean_typed_and_open_misc_writes_have_no_warnings() {
    let (state, _tmp) = make_fs_state().await;
    let clean = create_namespaced(
        State(Arc::clone(&state)),
        Path(("EdgionConfigData".to_string(), "default".to_string())),
        config_data_body("clean-cidrs", false),
    )
    .await
    .expect("clean typed ConfigData must be accepted");
    assert!(clean.2 .0.warnings.is_empty());

    let misc = Bytes::from(
        r#"{"apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{"name":"vendor-data","namespace":"default"},"spec":{"data":{"type":"Misc","config":{"vendor":{"arbitrary":[1,2,3]}}}}}"#,
    );
    let misc = create_namespaced(
        State(state),
        Path(("EdgionConfigData".to_string(), "default".to_string())),
        misc,
    )
    .await
    .expect("open Misc ConfigData must be accepted");
    assert!(misc.2 .0.warnings.is_empty());
}

#[tokio::test]
async fn link_sys_create_and_update_warn_without_rejecting() {
    let (state, _tmp) = make_fs_state().await;
    let created = create_namespaced(
        State(Arc::clone(&state)),
        Path(("LinkSys".to_string(), "default".to_string())),
        link_sys_body("redis", true),
    )
    .await
    .expect("LinkSys advisory must not reject create");
    assert_eq!(created.2 .0.warnings, ["config.timeout.conect"]);

    let updated = update_namespaced(
        State(state),
        Path(("LinkSys".to_string(), "default".to_string(), "redis".to_string())),
        HeaderMap::new(),
        link_sys_body("redis", true),
    )
    .await
    .expect("LinkSys advisory must not reject update");
    assert_eq!(updated.0.warnings, ["config.timeout.conect"]);
}

#[tokio::test]
async fn link_sys_clean_create_has_no_warnings() {
    let (state, _tmp) = make_fs_state().await;
    let (status, _headers, body) = create_namespaced(
        State(state),
        Path(("LinkSys".to_string(), "default".to_string())),
        link_sys_body("clean", false),
    )
    .await
    .expect("clean LinkSys must create successfully");
    assert_eq!(status, StatusCode::CREATED);
    assert!(body.0.warnings.is_empty());

    let serialized = serde_json::to_value(&body.0).expect("ApiResponse must serialize");
    assert!(serialized.get("warnings").is_none());
}
