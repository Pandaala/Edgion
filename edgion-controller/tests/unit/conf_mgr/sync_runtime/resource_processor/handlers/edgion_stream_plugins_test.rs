use super::*;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::EdgionStreamPlugins;
use std::sync::Arc;

// ── Cross-namespace ref-denial marking test (ESP) ───────────────────────

#[tokio::test]
async fn esp_cross_ns_gcir_deny_ref_without_grant_is_marked() {
    use edgion_resources::prelude_resources::EdgionConfigData;
    use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};

    // Build an EdgionStreamPlugins with a GCIR profile deny_ref pointing cross-ns.
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: gcir-esp
  namespace: team-a
spec:
  plugins:
    - type: GlobalConnectionIpRestriction
      config:
        enable: true
        activeProfile: main
        profiles:
          main:
            defaultAction: deny
            denyRefs:
              - name: blocklist
                namespace: team-b
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let ctx = make_ctx_with_runtime(Arc::new(ControllerRuntime::default()));

    // Seed the IpList in team-b (enabled, Namespace visibility -> requires grant).
    let mut ed = EdgionConfigData::new(
        "blocklist",
        EdgionConfigDataSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            enable: true,
            active: None,
            visibility: Default::default(), // Namespace
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec!["9.9.9.9/32".into()],
                }],
            }),
        },
    );
    ed.metadata.namespace = Some("team-b".to_string());
    let mut m = std::collections::HashMap::new();
    m.insert("team-b/blocklist".to_string(), ed);
    ctx.runtime().edgion_config_data_store.replace_all(m);
    // No ReferenceGrant -> must be denied.

    let handler = EdgionStreamPluginsHandler::new();
    let parsed = match handler.parse(esp, &ctx).await {
        ProcessResult::Continue(v) => v,
        other => panic!("expected Continue, got {:?}", other),
    };

    // Navigate to the deny_ref in the GCIR profile.
    let r = parsed
        .spec
        .plugins
        .as_ref()
        .and_then(|v| {
            v.iter()
                .find(|e| matches!(e.plugin, EdgionStreamPlugin::GlobalConnectionIpRestriction(_)))
        })
        .and_then(|e| match &e.plugin {
            EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                gcir.profiles.get("main")?.refs.deny_refs.as_ref()?.first()
            }
            _ => None,
        })
        .expect("missing GCIR profile deny_refs[0]");
    assert!(
        r.ref_denied.is_some(),
        "cross-ns GCIR deny_ref without grant must be marked denied; got ref_denied = {:?}",
        r.ref_denied
    );
    let denied = r.ref_denied.as_ref().unwrap();
    assert_eq!(denied.target_namespace, "team-b");
    assert_eq!(denied.target_name, "blocklist");
    assert_eq!(denied.reason.as_deref(), Some("NoMatchingReferenceGrant"));
}

#[test]
fn iter_ip_list_refs_collects_l4_gcir_and_tls_sorted() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: refs
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        denyRefs: [{ name: l4-blk }]
    - type: GlobalConnectionIpRestriction
      config:
        enable: true
        activeProfile: main
        profiles:
          main:
            defaultAction: deny
            allowRefs: [{ name: gcir-allow }]
  tlsRoutePlugins:
    - type: IpRestriction
      config:
        denyRefs: [{ name: tls-blk }]
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let refs = EdgionStreamPluginsHandler::iter_ip_list_refs(&esp);
    // Sorted by (ns, name): all default namespace.
    assert_eq!(
        refs,
        vec![
            ("default".to_string(), "gcir-allow".to_string()),
            ("default".to_string(), "l4-blk".to_string()),
            ("default".to_string(), "tls-blk".to_string()),
        ]
    );
}

#[tokio::test]
async fn stream_ip_list_refs_registered_and_resolved() {
    use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: refs-esp
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        denyRefs: [{ name: l4-blk }]
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let ctx = make_ctx();

    // Missing IpList -> one error, expected_type IpList.
    let errors = resolve_ip_list_refs(&esp, &ctx);
    assert_eq!(errors.len(), 1);
    match &errors[0] {
        ResolvedRefsError::EdgionConfigDataNotFound {
            name, expected_type, ..
        } => {
            assert_eq!(name, "l4-blk");
            assert_eq!(expected_type, "IpList");
        }
        other => panic!("expected EdgionConfigDataNotFound, got {:?}", other),
    }

    // parse registers the reverse ref for requeue.
    let handler = EdgionStreamPluginsHandler::new();
    let _ = handler.parse(esp.clone(), &ctx).await;
    let regs = ctx.runtime().edgion_config_data_refs.get_refs("default/l4-blk");
    assert!(
        regs.contains(&"EdgionStreamPlugins:default/refs-esp".to_string()),
        "got: {:?}",
        regs
    );

    // Seed the IpList -> resolves cleanly.
    let mut ed = EdgionConfigData::new(
        "l4-blk",
        EdgionConfigDataSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            enable: true,
            active: None,
            visibility: Default::default(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec!["9.9.9.9/32".into()],
                }],
            }),
        },
    );
    ed.metadata.namespace = Some("default".to_string());
    let mut m = std::collections::HashMap::new();
    m.insert("default/l4-blk".to_string(), ed);
    ctx.runtime().edgion_config_data_store.replace_all(m);
    assert!(resolve_ip_list_refs(&esp, &ctx).is_empty());
}

// ── Task 6: ESP cross_ns_ref_mgr registration test ──────────────────────

#[tokio::test]
async fn esp_cross_ns_ref_registered_for_grant_revalidation() {
    use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};

    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: cross-ns-esp
  namespace: team-a
spec:
  plugins:
    - type: IpRestriction
      config:
        denyRefs:
          - name: blocklist
            namespace: team-b
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let ctx = make_ctx_with_runtime(Arc::new(ControllerRuntime::default()));

    // Seed the IpList in team-b so ref_denied is set on parse.
    let mut ed = EdgionConfigData::new(
        "blocklist",
        EdgionConfigDataSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            enable: true,
            active: None,
            visibility: Default::default(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec!["9.9.9.9/32".into()],
                }],
            }),
        },
    );
    ed.metadata.namespace = Some("team-b".to_string());
    let mut m = std::collections::HashMap::new();
    m.insert("team-b/blocklist".to_string(), ed);
    ctx.runtime().edgion_config_data_store.replace_all(m);

    let handler = EdgionStreamPluginsHandler::new();
    let _ = handler.parse(esp, &ctx).await;

    // After parse(), cross_ns_ref_mgr must contain this ESP's ResourceRef under "team-b".
    let refs = ctx.runtime().cross_ns_ref_mgr.get_refs("team-b");
    assert!(
        refs.iter().any(|rr| rr.kind == ResourceKind::EdgionStreamPlugins),
        "ESP must be registered under team-b in cross_ns_ref_mgr; got: {:?}",
        refs
    );
}

#[tokio::test]
async fn esp_on_delete_clears_cross_ns_refs() {
    use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};

    // ESP in team-a with a cross-ns IpList deny_ref into team-b.
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: cross-ns-esp
  namespace: team-a
spec:
  plugins:
    - type: IpRestriction
      config:
        denyRefs:
          - name: blocklist
            namespace: team-b
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let ctx = make_ctx_with_runtime(Arc::new(ControllerRuntime::default()));

    // Seed the IpList in team-b so parse() registers the cross-ns ref.
    let mut ed = EdgionConfigData::new(
        "blocklist",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec!["9.9.9.9/32".into()],
                }],
            }),
        },
    );
    ed.metadata.namespace = Some("team-b".to_string());
    let mut m = std::collections::HashMap::new();
    m.insert("team-b/blocklist".to_string(), ed);
    ctx.runtime().edgion_config_data_store.replace_all(m);

    let handler = EdgionStreamPluginsHandler::new();
    // parse() registers the ESP under team-b in cross_ns_ref_mgr.
    let _ = handler.parse(esp.clone(), &ctx).await;

    let refs_before = ctx.runtime().cross_ns_ref_mgr.get_refs("team-b");
    assert!(
        refs_before
            .iter()
            .any(|rr| rr.kind == ResourceKind::EdgionStreamPlugins),
        "ESP must be registered under team-b in cross_ns_ref_mgr after parse; got: {:?}",
        refs_before
    );

    // on_delete() must clear the registration.
    handler.on_delete(&esp, &ctx).await;

    let refs_after = ctx.runtime().cross_ns_ref_mgr.get_refs("team-b");
    assert!(
        !refs_after.iter().any(|rr| rr.kind == ResourceKind::EdgionStreamPlugins),
        "ESP must not be in cross_ns_ref_mgr under team-b after on_delete; got: {:?}",
        refs_after
    );
}

fn make_ctx() -> HandlerContext {
    make_ctx_with_runtime(Arc::new(ControllerRuntime::default()))
}

fn make_ctx_with_runtime(runtime: Arc<ControllerRuntime>) -> HandlerContext {
    HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
}

fn advisory_yaml_esp() -> EdgionStreamPlugins {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: legacy
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        allowRefs:
          - name: default
        ipSource: DirectPeerIp
        message: blocked
        status: 403
"#;
    let raw: serde_json::Value = yaml_serde::from_str(yaml).unwrap();
    let mut esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    esp.spec.unknown_fields = edgion_resources::schema_check::unknown_key_paths(
        &raw["spec"],
        edgion_resources::resources::edgion_stream_plugins::edgion_stream_plugins_spec_schema(),
    );
    esp
}

fn clean_esp() -> EdgionStreamPlugins {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: clean
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        allowRefs:
          - name: default
        defaultAction: deny
"#;
    yaml_serde::from_str(yaml).unwrap()
}

#[test]
fn preparse_accepts_current_references_with_advisory_http_fields() {
    let mut esp = advisory_yaml_esp();
    assert_eq!(
        esp.spec.unknown_fields,
        vec![
            "plugins[0].config.ipSource",
            "plugins[0].config.message",
            "plugins[0].config.status"
        ]
    );
    assert!(EdgionStreamPluginsHandler::new()
        .preparse(&mut esp, &make_ctx())
        .is_empty());
}

fn invalid_esp() -> EdgionStreamPlugins {
    // Structurally invalid: IpRestriction with neither allowRefs nor denyRefs.
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: invalid
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        defaultAction: deny
"#;
    yaml_serde::from_str(yaml).unwrap()
}

#[test]
fn preparse_rejects_structurally_invalid_config() {
    let handler = EdgionStreamPluginsHandler::new();
    let mut esp = invalid_esp();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert_eq!(
        errors.len(),
        1,
        "expected one structural validation error; got: {errors:?}"
    );
    let e = &errors[0];
    assert!(e.contains("plugins[0] (IpRestriction)"));
    assert!(e.contains("allow") && e.contains("deny"));
}

#[test]
fn preparse_rejects_invalid_tls_route_plugin() {
    // Stage 2 has the same silent-blackhole risk as Stage 1: an invalid
    // tlsRoutePlugins config fails closed at L4 but must surface as Accepted=False.
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: invalid-tls
  namespace: default
spec:
  tlsRoutePlugins:
    - type: IpRestriction
      config:
        defaultAction: deny
"#;
    let mut esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert_eq!(
        errors.len(),
        1,
        "expected one Stage 2 validation error; got: {errors:?}"
    );
    let e = &errors[0];
    assert!(e.contains("tlsRoutePlugins[0] (IpRestriction)"), "got: {e}");
}

#[test]
fn preparse_skips_disabled_plugins() {
    // A parked (disabled) broken config must not flip Accepted to False, matching the
    // runtime builders which never construct a disabled plugin.
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: disabled
  namespace: default
spec:
  plugins:
    - enable: false
      type: IpRestriction
      config:
        defaultAction: deny
  tlsRoutePlugins:
    - enable: false
      type: IpRestriction
      config:
        defaultAction: deny
"#;
    let mut esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert!(errors.is_empty(), "disabled plugins must be skipped; got: {errors:?}");
}

#[test]
fn update_status_maps_structural_error_to_invalid_reason() {
    let handler = EdgionStreamPluginsHandler::new();
    let mut esp = invalid_esp();
    let errors = handler.preparse(&mut esp, &make_ctx());
    handler.update_status(&mut esp, &make_ctx(), &errors);

    let status = esp.status.as_ref().expect("status populated");
    let accepted = status
        .as_local()
        .expect("native test status")
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .expect("Accepted condition present");
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
}

#[test]
fn preparse_rejects_connection_rate_limit_without_dimension() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: t
  namespace: default
spec:
  plugins:
    - type: ConnectionRateLimit
      config:
        redisRef: "default/redis"
"#;
    let mut esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert_eq!(errors.len(), 1, "expected one validation error; got: {errors:?}");
    let e = &errors[0];
    assert!(e.contains("plugins[0] (ConnectionRateLimit)"), "got: {e}");
    assert!(e.contains("perListener") || e.contains("perSourceIp"), "got: {e}");
}

#[test]
fn preparse_validates_enabled_connection_limit_only() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: t
  namespace: default
spec:
  plugins:
    - type: ConnectionLimit
      config:
        maxConnections: 0
    - type: ConnectionLimit
      config:
        maxConnections: 100
    - type: ConnectionLimit
      enable: false
      config:
        maxConnections: 0
"#;
    let mut esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert_eq!(
        errors.len(),
        1,
        "only the enabled invalid entry is reported; got: {errors:?}"
    );
    assert!(errors[0].contains("plugins[0] (ConnectionLimit)"), "got: {}", errors[0]);
    assert!(errors[0].contains("maxConnections"), "got: {}", errors[0]);
}

#[test]
fn preparse_is_quiet_on_clean_config() {
    let handler = EdgionStreamPluginsHandler::new();
    let mut esp = clean_esp();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert!(
        errors.is_empty(),
        "clean config should not produce preparse errors; got: {:?}",
        errors
    );
}

#[test]
fn update_status_keeps_advisory_fields_separate_from_acceptance() {
    let handler = EdgionStreamPluginsHandler::new();
    let mut esp = advisory_yaml_esp();
    let errors = handler.preparse(&mut esp, &make_ctx());
    handler.update_status(&mut esp, &make_ctx(), &errors);
    let conditions = &esp.status.as_ref().unwrap().as_local().unwrap().conditions;
    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "True");
    assert_eq!(accepted.reason, "Accepted");
    let unknown = conditions.iter().find(|c| c.type_ == "UnknownConfigFields").unwrap();
    for path in &esp.spec.unknown_fields {
        assert!(unknown.message.contains(path));
    }
}

#[test]
fn inline_acl_remains_invalid_with_current_references_and_advisory_fields() {
    let mut esp: EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"inline","namespace":"default"},
        "spec":{"plugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"office"}],"allow":[],"message":"ignored","invalidInput":false}}]}
    })).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert_eq!(
        errors,
        vec!["plugins[0] (IpRestriction): Inline ACL allow/deny fields are not supported; use allowRefs/denyRefs"]
    );
    handler.update_status(&mut esp, &make_ctx(), &errors);
    let accepted = esp
        .status
        .as_ref()
        .unwrap()
        .as_local()
        .unwrap()
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
}

#[test]
fn update_status_uses_invalid_reason_for_non_legacy_errors() {
    let handler = EdgionStreamPluginsHandler::new();
    let mut esp = clean_esp();
    let errors = vec!["something else went wrong".to_string()];
    handler.update_status(&mut esp, &make_ctx(), &errors);

    let status = esp.status.as_ref().unwrap();
    let accepted = status
        .as_local()
        .expect("native test status")
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .unwrap();
    assert_eq!(accepted.reason, "Invalid");
}

// ── UnknownConfigFields condition ───────────────────────────────────────

#[test]
fn update_status_sets_unknown_config_fields_condition_with_exact_paths() {
    use crate::conf_mgr::sync_runtime::resource_processor::status_utils::condition_types;
    use edgion_resources::resources::edgion_stream_plugins::EdgionStreamPluginsSpec;

    let spec = EdgionStreamPluginsSpec {
        current_status: None,
        plugins: None,
        tls_route_plugins: None,
        unknown_fields: vec!["plugins[0].config.denys".to_string()],
    };
    let mut esp = EdgionStreamPlugins::new("typo-esp", spec);
    esp.metadata.namespace = Some("default".to_string());

    let handler = EdgionStreamPluginsHandler::new();
    handler.update_status(&mut esp, &make_ctx(), &[]);

    let status = esp.status.unwrap();
    let cond = status
        .as_local()
        .expect("native test status")
        .conditions
        .iter()
        .find(|c| c.type_ == condition_types::UNKNOWN_CONFIG_FIELDS)
        .expect("UnknownConfigFields condition must be present");
    assert_eq!(cond.status, "True");
    assert_eq!(cond.message, "plugins[0].config.denys");

    // Accepted must stay True and untouched by this advisory condition.
    let accepted = status
        .as_local()
        .expect("native test status")
        .conditions
        .iter()
        .find(|c| c.type_ == condition_types::ACCEPTED)
        .expect("Accepted condition must be present");
    assert_eq!(accepted.status, "True");
}

#[test]
fn update_status_removes_unknown_config_fields_condition_when_fixed() {
    // Simulates the operator fixing the typo on the next generation:
    // update_status must REMOVE the stale condition (the wedge trap the
    // stamp module's doc comment warns about).
    use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
        condition_reasons, condition_true, condition_types,
    };
    use edgion_resources::resources::common::ConditionsStatus;

    let mut esp = clean_esp();
    esp.spec.unknown_fields = Vec::new(); // fixed
    esp.status = Some(
        ConditionsStatus {
            conditions: vec![condition_true(
                condition_types::UNKNOWN_CONFIG_FIELDS,
                condition_reasons::UNKNOWN_FIELDS_DETECTED,
                "stale",
                Some(1),
            )],
        }
        .into(),
    );

    let handler = EdgionStreamPluginsHandler::new();
    handler.update_status(&mut esp, &make_ctx(), &[]);

    assert!(esp
        .status
        .unwrap()
        .as_local()
        .expect("native test status")
        .conditions
        .iter()
        .all(|c| c.type_ != condition_types::UNKNOWN_CONFIG_FIELDS));
}

// ── Inline entry-error collection tests ──────────────────────────────────

#[test]
fn collect_inline_ip_entry_errors_l4_bad_cidr() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: bad-inline
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        deny:
          - name: bad
            cidrs: ["not-a-cidr"]
        defaultAction: allow
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let mut esp = esp;
    let errs = EdgionStreamPluginsHandler::new().preparse(&mut esp, &make_ctx());
    assert_eq!(
        errs.len(),
        1,
        "expected typed invalid legacy L4 configuration; got: {:?}",
        errs
    );
}

#[test]
fn collect_inline_ip_entry_errors_gcir_bad_cidr() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: gcir-bad
  namespace: default
spec:
  plugins:
    - type: GlobalConnectionIpRestriction
      config:
        enable: true
        activeProfile: main
        profiles:
          main:
            defaultAction: deny
            allow:
              - name: bad
                cidrs: ["256.0.0.1/32"]
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let mut esp = esp;
    let errs = EdgionStreamPluginsHandler::new().preparse(&mut esp, &make_ctx());
    assert_eq!(
        errs.len(),
        1,
        "expected typed invalid legacy GCIR configuration; got: {:?}",
        errs
    );
}

#[test]
fn collect_inline_ip_entry_errors_tls_route_bad_cidr() {
    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tls-bad
  namespace: default
spec:
  tlsRoutePlugins:
    - type: IpRestriction
      config:
        deny:
          - name: bad
            cidrs: ["not-a-cidr"]
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let mut esp = esp;
    let errs = EdgionStreamPluginsHandler::new().preparse(&mut esp, &make_ctx());
    assert_eq!(
        errs.len(),
        1,
        "expected typed invalid legacy TLS configuration; got: {:?}",
        errs
    );
}

#[test]
fn collect_inline_ip_entry_errors_empty_for_clean_config() {
    let esp = clean_esp();
    let mut esp = esp;
    let errs = EdgionStreamPluginsHandler::new().preparse(&mut esp, &make_ctx());
    assert!(errs.is_empty(), "expected no errors for clean config; got: {:?}", errs);
}

// ── RefNotPermitted status reporting test (Task 5) ──────────────────────

#[tokio::test]
async fn esp_denied_ref_reports_ref_not_permitted() {
    // Cross-namespace GCIR profile deny_ref with no ReferenceGrant -> after parse() marks
    // ref_denied, update_status must report ResolvedRefs=False reason=RefNotPermitted.
    use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};

    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: gcir-esp
  namespace: team-a
spec:
  plugins:
    - type: GlobalConnectionIpRestriction
      config:
        enable: true
        activeProfile: main
        profiles:
          main:
            defaultAction: deny
            denyRefs:
              - name: blocklist
                namespace: team-b
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let ctx = make_ctx_with_runtime(Arc::new(ControllerRuntime::default()));

    // Seed the IpList in team-b (enabled, Namespace visibility -> requires grant).
    let mut ed = EdgionConfigData::new(
        "blocklist",
        EdgionConfigDataSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            enable: true,
            active: None,
            visibility: Default::default(), // Namespace
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec!["9.9.9.9/32".into()],
                }],
            }),
        },
    );
    ed.metadata.namespace = Some("team-b".to_string());
    let mut m = std::collections::HashMap::new();
    m.insert("team-b/blocklist".to_string(), ed);
    ctx.runtime().edgion_config_data_store.replace_all(m);
    // No ReferenceGrant -> parse() marks ref_denied on the deny_ref.

    let handler = EdgionStreamPluginsHandler::new();
    let mut parsed = match handler.parse(esp, &ctx).await {
        ProcessResult::Continue(v) => v,
        other => panic!("expected Continue, got {:?}", other),
    };

    handler.update_status(&mut parsed, &ctx, &[]);

    let status = parsed
        .status
        .as_ref()
        .expect("status must be populated by update_status");
    let cond = status
        .as_local()
        .expect("native test status")
        .conditions
        .iter()
        .find(|c| c.type_ == "ResolvedRefs")
        .expect("ResolvedRefs condition must be present");
    assert_eq!(cond.status, "False");
    assert_eq!(
        cond.reason, "RefNotPermitted",
        "denied cross-ns ESP ref must yield reason RefNotPermitted"
    );
}

#[tokio::test]
async fn stream_disabled_ip_list_ref_reports_error() {
    use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};

    let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: disabled-ref-esp
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        denyRefs: [{ name: disabled-list }]
"#;
    let esp: EdgionStreamPlugins = yaml_serde::from_str(yaml).unwrap();
    let ctx = make_ctx();

    // Seed a disabled (enable: false) IpList -> must not resolve.
    let mut ed = EdgionConfigData::new(
        "disabled-list",
        EdgionConfigDataSpec {
            current_status: None,
            enable: false,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "g".into(),
                    description: None,
                    cidrs: vec!["1.2.3.4/32".into()],
                }],
            }),
        },
    );
    ed.metadata.namespace = Some("default".to_string());
    let mut m = std::collections::HashMap::new();
    m.insert("default/disabled-list".to_string(), ed);
    ctx.runtime().edgion_config_data_store.replace_all(m);

    let errors = resolve_ip_list_refs(&esp, &ctx);
    assert_eq!(errors.len(), 1, "disabled IpList must not resolve; got: {:?}", errors);
    match &errors[0] {
        ResolvedRefsError::EdgionConfigDataNotFound {
            name, expected_type, ..
        } => {
            assert_eq!(name, "disabled-list");
            assert_eq!(expected_type, "IpList");
        }
        other => panic!("expected EdgionConfigDataNotFound, got {:?}", other),
    }
}

#[test]
fn unsupported_stream_types_report_invalid_status_and_survive_transport() {
    for stage in ["plugins", "tlsRoutePlugins"] {
        for enabled in [true, false] {
            let mut esp: EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
                "apiVersion":"edgion.io/v1", "kind":"EdgionStreamPlugins",
                "metadata":{"name":"unknown", "namespace":"default"},
                "spec":{stage:[{"type":"FuturePlugin", "enable":enabled, "config":{"token":"discard-me"}}]}
            }))
            .unwrap();
            let errors = EdgionStreamPluginsHandler::new().preparse(&mut esp, &make_ctx());
            assert_eq!(errors, vec![format!("{stage}[0]: unsupported plugin type")]);
            EdgionStreamPluginsHandler::new().update_status(&mut esp, &make_ctx(), &errors);
            let status = serde_json::to_value(&esp.status).unwrap();
            let accepted = status["conditions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["type"] == "Accepted")
                .unwrap();
            assert_eq!(accepted["status"], "False");
            let encoded = serde_json::to_string(&esp).unwrap();
            assert!(!encoded.contains("discard-me"));
            let restored: EdgionStreamPlugins = serde_json::from_str(&encoded).unwrap();
            assert_eq!(restored.unsupported_plugin_errors(), errors);
        }
    }
}

#[tokio::test]
async fn all_standalone_stream_slots_keep_raw_bare_owner_and_valid_recovery_edges() {
    let ctx = make_ctx();
    let owner: EdgionStreamPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"bare-ip"},"spec":{
        "plugins":[{"type":"IpRestriction","enable":false,"config":{"allowRefs":[{"name":"l4"},{"name":"bad/name"}]}},
          {"type":"GlobalConnectionIpRestriction","config":{"enable":true,"activeProfile":"main","profiles":{"main":{"denyRefs":[{"name":"profile"},{"name":"bad","namespace":""}]}}}}],
        "tlsRoutePlugins":[{"type":"IpRestriction","enable":false,"config":{"denyRefs":[{"name":"tls"}]}}]
    }})).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let ProcessResult::Continue(parsed) = handler.parse(owner.clone(), &ctx).await else {
        panic!()
    };
    for name in ["l4", "profile", "tls"] {
        assert_eq!(
            ctx.runtime()
                .edgion_config_data_refs
                .get_refs(&format!("default/{name}")),
            ["EdgionStreamPlugins:bare-ip"]
        );
    }
    for key in ["default/bad/name", "/bad"] {
        assert!(ctx.runtime().edgion_config_data_refs.get_refs(key).is_empty());
    }
    assert_eq!(
        resolve_ip_list_refs(&parsed, &ctx)
            .iter()
            .filter(|error| matches!(error, ResolvedRefsError::InvalidReference { .. }))
            .count(),
        2
    );
    let mut next = owner;
    next.spec.plugins = None;
    next.spec.tls_route_plugins =
        serde_json::from_value(serde_json::json!([{"type":"IpRestriction","config":{"allowRefs":[{"name":"next"}]}}]))
            .unwrap();
    let ProcessResult::Continue(parsed) = handler.parse(next, &ctx).await else {
        panic!()
    };
    for name in ["l4", "profile", "tls"] {
        assert!(ctx
            .runtime()
            .edgion_config_data_refs
            .get_refs(&format!("default/{name}"))
            .is_empty());
    }
    assert_eq!(
        ctx.runtime().edgion_config_data_refs.get_refs("default/next"),
        ["EdgionStreamPlugins:bare-ip"]
    );
    let target: edgion_resources::EdgionConfigData = serde_json::from_value(serde_json::json!({"metadata":{"name":"next","namespace":"default"},"spec":{"data":{"type":"IpList","config":{"items":[]}}}})).unwrap();
    ctx.runtime()
        .edgion_config_data_store
        .replace_all(std::collections::HashMap::from([("default/next".into(), target)]));
    assert!(resolve_ip_list_refs(&parsed, &ctx).is_empty());
    handler.on_delete(&parsed, &ctx).await;
    assert!(ctx
        .runtime()
        .edgion_config_data_refs
        .get_refs("default/next")
        .is_empty());
}

#[tokio::test]
async fn standalone_stream_grant_source_isolation_add_and_revoke_preserve_edges() {
    use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
    let ctx = make_ctx();
    let target: edgion_resources::EdgionConfigData=serde_json::from_value(serde_json::json!({"metadata":{"name":"nets","namespace":"ops"},"spec":{"enable":true,"data":{"type":"IpList","config":{"items":[]}}}})).unwrap();
    ctx.runtime()
        .edgion_config_data_store
        .replace_all(std::collections::HashMap::from([("ops/nets".into(), target)]));
    let owner: EdgionStreamPlugins=serde_json::from_value(serde_json::json!({"metadata":{"name":"bare-grant-ip"},"spec":{
        "plugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":"nets","namespace":"ops"}]}},
          {"type":"GlobalConnectionIpRestriction","config":{"enable":true,"activeProfile":"main","profiles":{"main":{"denyRefs":[{"name":"nets","namespace":"ops"}]}}}}],
        "tlsRoutePlugins":[{"type":"IpRestriction","config":{"denyRefs":[{"name":"nets","namespace":"ops"}]}}]
    }})).unwrap();
    let raw = ResourceRef::new(ResourceKind::EdgionStreamPlugins, None, "bare-grant-ip".into());
    let handler = EdgionStreamPluginsHandler::new();
    for from in [None, Some("EdgionPlugins"), Some("EdgionStreamPlugins"), None] {
        ctx.runtime().ref_grant_store.remove("ops/r5c2-grant");
        if let Some(kind) = from {
            let grant=serde_json::from_value(serde_json::json!({"metadata":{"name":"r5c2-grant","namespace":"ops"},"spec":{"from":[{"group":"edgion.io","kind":kind,"namespace":"default"}],"to":[{"group":"edgion.io","kind":"EdgionConfigData","name":"nets"}]}})).unwrap();
            ctx.runtime()
                .ref_grant_store
                .upsert("ops/r5c2-grant".into(), Arc::new(grant));
        }
        let allowed = from == Some("EdgionStreamPlugins");
        let ProcessResult::Continue(mut parsed) = handler.parse(owner.clone(), &ctx).await else {
            panic!()
        };
        let prepared = PreparedIpListReferences::from_stream_plugins(&parsed);
        let binding = prepared.bind_stream_plugins(&parsed).unwrap();
        let markers: Vec<_> = binding
            .references()
            .flat_map(|refs| {
                refs.allow()
                    .chain(refs.deny())
                    .map(|(_, denied)| denied)
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(markers, vec![!allowed; 3]);
        let errors = resolve_ip_list_refs(&parsed, &ctx);
        assert_eq!(errors.is_empty(), allowed);
        assert!(errors.iter().all(|e| matches!(
            e,
            ResolvedRefsError::RefNotPermitted {
                from_kind: "EdgionStreamPlugins",
                ..
            }
        )));
        handler.update_status(&mut parsed, &ctx, &[]);
        assert_eq!(
            ctx.runtime().edgion_config_data_refs.get_refs("ops/nets"),
            ["EdgionStreamPlugins:bare-grant-ip"]
        );
        assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("ops").contains(&raw));
        let resolved = parsed
            .status
            .as_ref()
            .expect("status must be published")
            .as_local()
            .expect("native status")
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .expect("ResolvedRefs condition");
        assert_eq!(resolved.status, if allowed { "True" } else { "False" });
        assert_eq!(
            resolved.reason,
            if allowed { "ResolvedRefs" } else { "RefNotPermitted" }
        );
    }
    handler.on_delete(&owner, &ctx).await;
    assert!(ctx.runtime().edgion_config_data_refs.get_refs("ops/nets").is_empty());
    assert!(!ctx.runtime().cross_ns_ref_mgr.get_refs("ops").contains(&raw));
}

#[tokio::test]
async fn gcir_selector_actual_source_grants_and_default_namespace_are_current() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::config_data_ref_mark::resolve_field;
    use edgion_resources::refs::config_data_fields::StreamConfigDataEntry;
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = make_ctx_with_runtime(runtime.clone());
    let mut owner: EdgionStreamPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"bare-gcir"},"spec":{"plugins":[{
        "type":"GlobalConnectionIpRestriction","enable":false,"config":{"enable":true,"activeProfile":"base","activeProfileRef":{"name":"selector"},"profiles":{"base":{}}}
    }]}})).unwrap();
    assert_eq!(
        EdgionStreamPluginsHandler::iter_gcir_refs(&owner).collect::<Vec<_>>(),
        vec![("default".into(), "selector".into())]
    );
    let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) = &mut owner.spec.plugins.as_mut().unwrap()[0].plugin
    else {
        unreachable!()
    };
    config.active_profile_ref.as_mut().unwrap().namespace = Some("shared".into());
    let target=serde_json::from_value(serde_json::json!({"metadata":{"name":"selector","namespace":"shared"},"spec":{"data":{"type":"Selector","config":{"active":"base"}}}})).unwrap();
    runtime
        .edgion_config_data_store
        .replace_all(std::collections::HashMap::from([("shared/selector".into(), target)]));
    for source in [Some("EdgionPlugins"), Some("EdgionStreamPlugins"), None] {
        runtime.ref_grant_store.remove("shared/selector");
        if let Some(source) = source {
            let grant=serde_json::from_value(serde_json::json!({"metadata":{"name":"selector","namespace":"shared"},"spec":{
                "from":[{"group":"edgion.io","kind":source,"namespace":"default"}],"to":[{"group":"edgion.io","kind":"EdgionConfigData","name":"selector"}]
            }})).unwrap();
            runtime
                .ref_grant_store
                .upsert("shared/selector".into(), Arc::new(grant));
        }
        let loan = StreamConfigDataEntry::from_stream_plugins(&owner, 0)
            .unwrap()
            .field()
            .unwrap();
        assert_eq!(
            resolve_field(&loan, &ctx).error.is_none(),
            source == Some("EdgionStreamPlugins")
        );
        assert_eq!(
            EdgionStreamPluginsHandler::iter_gcir_refs(&owner).collect::<Vec<_>>(),
            vec![("shared".into(), "selector".into())]
        );
        let allowed = source == Some("EdgionStreamPlugins");
        let handler = EdgionStreamPluginsHandler::new();
        let ProcessResult::Continue(mut parsed) = handler.parse(owner.clone(), &ctx).await else {
            panic!("parse")
        };
        let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
            &parsed.spec.plugins.as_ref().unwrap()[0].plugin
        else {
            unreachable!()
        };
        assert_eq!(
            config.active_profile_ref.as_ref().unwrap().ref_denied.is_none(),
            allowed
        );
        assert_eq!(resolve_refs(&parsed, &ctx).is_empty(), allowed);
        handler.update_status(&mut parsed, &ctx, &[]);
        let resolved = parsed
            .status
            .as_ref()
            .unwrap()
            .as_local()
            .unwrap()
            .conditions
            .iter()
            .find(|c| c.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(resolved.status, if allowed { "True" } else { "False" });
        assert_eq!(
            resolved.reason,
            if allowed { "ResolvedRefs" } else { "RefNotPermitted" }
        );
        assert_eq!(
            runtime.edgion_config_data_refs.get_refs("shared/selector"),
            ["EdgionStreamPlugins:bare-gcir"]
        );
        let raw = ResourceRef::new(ResourceKind::EdgionStreamPlugins, None, "bare-gcir".into());
        assert!(runtime.cross_ns_ref_mgr.get_refs("shared").contains(&raw));
        let EdgionStreamPlugin::GlobalConnectionIpRestriction(config) =
            &mut parsed.spec.plugins.as_mut().unwrap()[0].plugin
        else {
            unreachable!()
        };
        config.active_profile_ref.as_mut().unwrap().ref_denied = None;
        assert_eq!(
            resolve_refs(&parsed, &ctx).is_empty(),
            allowed,
            "readonly permission ignores stale marker"
        );
        handler.on_delete(&parsed, &ctx).await;
        assert!(runtime.edgion_config_data_refs.get_refs("shared/selector").is_empty());
        assert!(!runtime.cross_ns_ref_mgr.get_refs("shared").contains(&raw));
    }
    owner.metadata.namespace = Some(String::new());
    assert!(EdgionStreamPluginsHandler::iter_gcir_refs(&owner)
        .collect::<Vec<_>>()
        .is_empty());
}

#[tokio::test]
async fn geo_stream_pass_preserves_acl_selector_edges_and_intrinsic_status() {
    use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = make_ctx_with_runtime(runtime.clone());
    let owner:EdgionStreamPlugins=serde_json::from_value(serde_json::json!({"metadata":{"name":"mixed-geo","namespace":"apps"},"spec":{"plugins":[
        {"type":"IpRestriction","config":{"allowRefs":[{"name":"acl"}]}},
        {"type":"GlobalConnectionIpRestriction","config":{"enable":true,"activeProfile":"main","profiles":{"main":{"allowRefs":[{"name":"profile"}]}}}},
        {"type":"GeoIpLocation","config":{"failOpen":true,"allowRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"geo"}]}]}]}}
    ]}})).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let ProcessResult::Continue(mut parsed) = handler.parse(owner, &ctx).await else {
        panic!()
    };
    for name in ["acl", "profile", "geo"] {
        assert_eq!(
            runtime.edgion_config_data_refs.get_refs(&format!("apps/{name}")),
            ["EdgionStreamPlugins:apps/mixed-geo"]
        );
    }
    PreparedIpListReferences::from_stream_plugins(&parsed)
        .visit_geo_stream_plugins(&parsed, |leaf| assert!(leaf.config().resolved_refs.is_some()))
        .unwrap();
    parsed.spec.plugins.as_mut().unwrap()[2].enable = false;
    let ProcessResult::Continue(parsed) = handler.parse(parsed, &ctx).await else {
        panic!()
    };
    assert!(runtime.edgion_config_data_refs.get_refs("apps/geo").is_empty());
    assert!(!runtime.edgion_config_data_refs.get_refs("apps/acl").is_empty());
    PreparedIpListReferences::from_stream_plugins(&parsed)
        .visit_geo_stream_plugins(&parsed, |leaf| assert!(leaf.config().resolved_refs.is_none()))
        .unwrap();
}

#[tokio::test]
async fn inactive_acl_sites_keep_passive_diagnostics_and_clear_all_executable_metadata() {
    use edgion_resources::refs::ip_list_fields::{IpListSide, PreparedIpListReferences};
    use edgion_resources::resources::edgion_plugins::ip_reference_set::IpReadScope;
    use serde_json::json;
    for family in ["connection", "outer-gcir", "inner-gcir", "tls"] {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = make_ctx_with_runtime(runtime.clone());
        let target:edgion_resources::EdgionConfigData=serde_json::from_value(json!({"metadata":{"name":"nets","namespace":"shared"},"spec":{"data":{"type":"IpList","config":{"items":[]}}}})).unwrap();
        runtime
            .edgion_config_data_store
            .replace_all(std::collections::HashMap::from([("shared/nets".into(), target)]));
        let reference =
            json!({"name":"nets","namespace":"shared","refDenied":{"targetNamespace":"shared","targetName":"nets"}});
        let refs = json!({"allowRefs":[reference,{"name":"bad/name"}]});
        let entry = if family.contains("gcir") {
            json!({"type":"GlobalConnectionIpRestriction","enable":family!="outer-gcir","config":{"enable":family!="inner-gcir","activeProfile":"base","activeProfileRef":{"name":"selector"},"profiles":{"base":refs}}})
        } else {
            json!({"type":"IpRestriction","enable":false,"config":refs})
        };
        let name = format!("inactive-{family}");
        let key = format!("EdgionStreamPlugins:apps/{name}");
        let mut spec = json!({"plugins":[{"type":"GeoIpLocation","config":{"failOpen":true,"allowRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"geo"}]}]}]}}]});
        if family == "tls" {
            spec["tlsRoutePlugins"] = json!([entry]);
        } else {
            spec["plugins"].as_array_mut().unwrap().push(entry);
        }
        let mut owner: EdgionStreamPlugins =
            serde_json::from_value(json!({"metadata":{"name":name,"namespace":"apps"},"spec":spec})).unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        prepared
            .visit_acl_stream_plugins_mut(&mut owner, |mut leaf| {
                assert!(!leaf.is_supported());
                leaf.set_read_scopes(IpListSide::Allow, &[IpReadScope::GrantAuthorized, IpReadScope::Denied])
                    .unwrap();
            })
            .unwrap();
        for allowed in [false, true] {
            runtime.ref_grant_store.remove("shared/permit");
            if allowed {
                let grant=serde_json::from_value(json!({"metadata":{"name":"permit","namespace":"shared"},"spec":{"from":[{"group":"edgion.io","kind":"EdgionStreamPlugins","namespace":"apps"}],"to":[{"group":"edgion.io","kind":"EdgionConfigData","name":"nets"}]}})).unwrap();
                runtime.ref_grant_store.upsert("shared/permit".into(), Arc::new(grant));
            }
            let handler = EdgionStreamPluginsHandler::new();
            let ProcessResult::Continue(parsed) = handler.parse(owner.clone(), &ctx).await else {
                panic!()
            };
            assert_eq!(runtime.edgion_config_data_refs.get_refs("shared/nets"), [key.as_str()]);
            assert_eq!(runtime.edgion_config_data_refs.get_refs("apps/geo"), [key.as_str()]);
            if family.contains("gcir") {
                assert_eq!(
                    runtime.edgion_config_data_refs.get_refs("apps/selector"),
                    [key.as_str()]
                );
            }
            assert!(runtime.edgion_config_data_refs.get_refs("apps/bad/name").is_empty());
            let prepared = PreparedIpListReferences::from_stream_plugins(&parsed);
            let binding = prepared.bind_stream_plugins(&parsed).unwrap();
            for leaf in binding.references() {
                assert!(leaf.config().resolved_allow_refs.is_none());
                assert!(leaf.config().resolved_deny_refs.is_none());
                assert!(leaf.allow().chain(leaf.deny()).all(|(_, denied)| !denied));
            }
            let errors = super::resolve_ip_list_refs(&parsed, &ctx);
            assert!(errors
                .iter()
                .any(|error| matches!(error, ResolvedRefsError::InvalidReference { .. })));
            let denied = errors
                .iter()
                .any(|error| matches!(error, ResolvedRefsError::RefNotPermitted { .. }));
            assert_eq!(
                denied,
                !allowed && crate::config::is_reference_grant_validation_enabled(),
                "{family}"
            );
        }
        let mut wire = serde_json::to_value(&owner).unwrap();
        let path = if family == "tls" {
            &mut wire["spec"]["tlsRoutePlugins"][0]["config"]
        } else if family.contains("gcir") {
            &mut wire["spec"]["plugins"][1]["config"]["profiles"]["base"]
        } else {
            &mut wire["spec"]["plugins"][1]["config"]
        };
        path["allowRefs"] = json!([{"name":"next"}]);
        path.as_object_mut().unwrap().remove("resolvedAllowRefs");
        let changed: EdgionStreamPlugins = serde_json::from_value(wire).unwrap();
        let handler = EdgionStreamPluginsHandler::new();
        let ProcessResult::Continue(parsed) = handler.parse(changed, &ctx).await else {
            panic!()
        };
        assert!(runtime.edgion_config_data_refs.get_refs("shared/nets").is_empty());
        assert_eq!(runtime.edgion_config_data_refs.get_refs("apps/next"), [key.as_str()]);
        assert_eq!(runtime.edgion_config_data_refs.get_refs("apps/geo"), [key.as_str()]);
        handler.on_delete(&parsed, &ctx).await;
        for target in ["apps/next", "apps/geo", "apps/selector"] {
            assert!(runtime.edgion_config_data_refs.get_refs(target).is_empty());
        }
    }
}
