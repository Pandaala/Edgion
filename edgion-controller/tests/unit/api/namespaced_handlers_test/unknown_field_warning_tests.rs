use super::update_identity_tests::make_fs_state;
use super::*;

/// `config.denyRefss` is a typo for `config.denyRefs` on `IpRestrictionConfig`.
fn plugins_body_with_typo(name: &str, namespace: &str) -> Bytes {
    Bytes::from(format!(
        r#"{{"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{{"name":"{name}","namespace":"{namespace}"}},"spec":{{"requestPlugins":[{{"type":"IpRestriction","config":{{"defaultAction":"allow","denyRefss":[]}}}}]}}}}"#
    ))
}

fn plugins_body_clean(name: &str, namespace: &str) -> Bytes {
    Bytes::from(format!(
        r#"{{"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{{"name":"{name}","namespace":"{namespace}"}},"spec":{{"requestPlugins":[{{"type":"IpRestriction","config":{{"defaultAction":"allow","denyRefs":[]}}}}]}}}}"#
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
    assert_eq!(body.0.warnings, vec!["requestPlugins[0].config.denyRefss".to_string()]);

    let serialized = serde_json::to_value(&body.0).expect("ApiResponse must serialize");
    assert_eq!(
        serialized["warnings"][0], "requestPlugins[0].config.denyRefss",
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
    assert_eq!(body.0.warnings, vec!["requestPlugins[0].config.denyRefss".to_string()]);
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

#[tokio::test]
async fn unsupported_extensions_create_and_update_preserve_required_config_envelope() {
    use crate::conf_mgr::SchemaValidator;
    use edgion_resources::resources::edgion_config_data::EdgionConfigData;
    use edgion_resources::resources::link_sys::LinkSys;
    use edgion_resources::ResourceKind;
    use serde_json::json;

    let (mut state, _tmp) = make_fs_state().await;
    let crd_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    Arc::get_mut(&mut state).unwrap().schema_validator =
        Arc::new(SchemaValidator::from_crd_dir(&crd_dir).expect("committed CRD schemas"));
    for (kind, config_path, expected_tag) in [
        (ResourceKind::LinkSys, "/spec/config", "unsupported"),
        (ResourceKind::EdgionConfigData, "/spec/data/config", "Unsupported"),
    ] {
        let extension = json!({"type":"FuturePrivateExtension","config":{"password":"opaque-secret-must-disappear"}});
        let spec = if kind == ResourceKind::LinkSys {
            extension
        } else {
            json!({"data":extension})
        };
        let raw = json!({
            "apiVersion":"edgion.io/v1", "kind":kind.as_str(),
            "metadata":{"name":"unknown-extension","namespace":"default"}, "spec":spec
        });
        let normalized = if kind == ResourceKind::LinkSys {
            let resource: LinkSys = serde_json::from_value(raw.clone()).unwrap();
            assert!(!resource.validate_config().is_empty());
            serde_json::to_value(resource).unwrap()
        } else {
            let resource: EdgionConfigData = serde_json::from_value(raw.clone()).unwrap();
            assert!(!resource.validate_pre_parse().is_empty());
            serde_json::to_value(resource).unwrap()
        };
        assert_eq!(normalized.pointer(config_path), Some(&json!({})));
        let type_path = config_path.replace("/config", "/type");
        assert_eq!(normalized.pointer(&type_path), Some(&json!(expected_tag)));
        assert!(!normalized.to_string().contains("opaque-secret-must-disappear"));
        state
            .schema_validator
            .validate(kind, &normalized)
            .expect("normalized unknown extension remains CRD-admissible");
        let mut missing_config = normalized.clone();
        let parent_path = config_path.strip_suffix("/config").unwrap();
        missing_config
            .pointer_mut(parent_path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("config");
        assert!(
            state.schema_validator.validate(kind, &missing_config).is_err(),
            "guard exercises the actual required-config schema"
        );

        let body = Bytes::from(serde_json::to_vec(&raw).unwrap());
        let (status, _, created) = create_namespaced(
            State(Arc::clone(&state)),
            Path((kind.as_str().to_string(), "default".to_string())),
            body.clone(),
        )
        .await
        .expect("unknown extension must reach processing instead of rejecting the normalized write");
        assert_eq!(status, StatusCode::CREATED);
        assert!(created.0.warnings.is_empty());
        let updated = update_namespaced(
            State(Arc::clone(&state)),
            Path((
                kind.as_str().to_string(),
                "default".to_string(),
                "unknown-extension".to_string(),
            )),
            HeaderMap::new(),
            body,
        )
        .await
        .expect("unknown extension update must reach processing and invalidate old state");
        assert!(updated.0.warnings.is_empty());
    }
}

#[tokio::test]
async fn undecodable_http_plugin_config_never_replaces_the_authored_source() {
    let (mut state, tmp) = make_fs_state().await;
    let crds = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    Arc::get_mut(&mut state).unwrap().schema_validator =
        Arc::new(crate::conf_mgr::SchemaValidator::from_crd_dir(&crds).unwrap());
    // The shared request-stage CRD declares KeyAuth.hideCredentials as a boolean.
    // The same key is advisory for IpRestriction, which does not consume it.
    let advisory = serde_json::json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
        "metadata":{"name":"advisory","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"IpRestriction","config":{"defaultAction":"allow","hideCredentials":"unknown"}}]}
    });
    let (_, _, result) = create_namespaced(
        State(state.clone()),
        Path(("EdgionPlugins".into(), "default".into())),
        Bytes::from(advisory.to_string()),
    )
    .await
    .unwrap();
    assert_eq!(result.0.warnings, vec!["requestPlugins[0].config.hideCredentials"]);
    let file = tmp.path().join("default/EdgionPlugins_default_auth-chain.yaml");
    let malformed = |stage: &str, config: serde_json::Value| {
        Bytes::from(
            serde_json::json!({
                "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
                "metadata":{"name":"auth-chain","namespace":"default"},
                "spec":{stage:[{"type":"KeyAuth","enable":false,"config":config}]}
            })
            .to_string(),
        )
    };
    let create_path = || Path(("EdgionPlugins".into(), "default".into()));
    let update_path = || Path(("EdgionPlugins".into(), "default".into(), "auth-chain".into()));
    let structurally_invalid = serde_json::json!({"hideCredentials":"bad"});
    assert_eq!(
        create_namespaced(
            State(state.clone()),
            create_path(),
            malformed("requestPlugins", structurally_invalid.clone())
        )
        .await
        .err(),
        Some(StatusCode::BAD_REQUEST)
    );
    assert!(!file.exists());
    let _ = create_namespaced(
        State(state.clone()),
        create_path(),
        plugins_body_clean("auth-chain", "default"),
    )
    .await
    .unwrap();
    let original = tokio::fs::read(&file).await.unwrap();
    for stage in [
        "requestPlugins",
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        assert_eq!(
            update_namespaced(
                State(state.clone()),
                update_path(),
                HeaderMap::new(),
                malformed(stage, structurally_invalid.clone())
            )
            .await
            .err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(tokio::fs::read(&file).await.unwrap(), original);
    }
}
