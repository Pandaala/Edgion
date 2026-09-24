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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
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
                gcir.profiles.get("main")?.deny_refs.as_ref()?.first()
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
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

fn legacy_yaml_esp() -> EdgionStreamPlugins {
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
        allow:
          - name: default
            cidrs: ["10.0.0.0/8"]
        ipSource: DirectPeerIp
        message: blocked
        status: 403
"#;
    serde_yaml::from_str(yaml).expect("legacy yaml must now parse (detection moved to preparse)")
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
        allow:
          - name: default
            cidrs: ["10.0.0.0/8"]
        defaultAction: deny
"#;
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn preparse_emits_legacy_marker_when_old_fields_present() {
    let handler = EdgionStreamPluginsHandler::new();
    let mut esp = legacy_yaml_esp();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert_eq!(errors.len(), 1);
    let e = &errors[0];
    assert!(
        e.starts_with(LEGACY_SCHEMA_MARKER),
        "preparse error must carry LEGACY_SCHEMA_DEPRECATED marker so update_status maps the reason; got: {e}"
    );
    assert!(e.contains("ipSource"));
    assert!(e.contains("message"));
    assert!(e.contains("status"));
    assert!(e.contains(MIGRATION_DOC_PATH));
}

fn invalid_esp() -> EdgionStreamPlugins {
    // Structurally invalid: IpRestriction with neither `allow` nor `deny`.
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
    serde_yaml::from_str(yaml).unwrap()
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
    assert!(
        !e.starts_with(LEGACY_SCHEMA_MARKER),
        "structural error must not carry the legacy marker so update_status uses the Invalid reason; got: {e}"
    );
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
    let mut esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert_eq!(
        errors.len(),
        1,
        "expected one Stage 2 validation error; got: {errors:?}"
    );
    let e = &errors[0];
    assert!(!e.starts_with(LEGACY_SCHEMA_MARKER));
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
    let mut esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
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
    let mut esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
    let handler = EdgionStreamPluginsHandler::new();
    let errors = handler.preparse(&mut esp, &make_ctx());
    assert_eq!(errors.len(), 1, "expected one validation error; got: {errors:?}");
    let e = &errors[0];
    assert!(
        !e.starts_with(LEGACY_SCHEMA_MARKER),
        "ConnectionRateLimit error must not carry legacy marker; got: {e}"
    );
    assert!(e.contains("plugins[0] (ConnectionRateLimit)"), "got: {e}");
    assert!(e.contains("perListener") || e.contains("perSourceIp"), "got: {e}");
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
fn update_status_maps_legacy_marker_to_dedicated_reason() {
    let handler = EdgionStreamPluginsHandler::new();
    let mut esp = legacy_yaml_esp();
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
    assert_eq!(accepted.reason, REASON_LEGACY_SCHEMA_DEPRECATED);
    // Message must not leak the internal marker prefix.
    assert!(!accepted.message.contains(LEGACY_SCHEMA_MARKER));
    assert!(accepted.message.contains("legacy HTTP fields"));
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
    let errs = collect_inline_ip_entry_errors(&esp);
    assert_eq!(
        errs.len(),
        1,
        "expected one entry error for bad L4 inline CIDR; got: {:?}",
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
    let errs = collect_inline_ip_entry_errors(&esp);
    assert_eq!(
        errs.len(),
        1,
        "expected one entry error for bad GCIR profile CIDR; got: {:?}",
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
    let errs = collect_inline_ip_entry_errors(&esp);
    assert_eq!(
        errs.len(),
        1,
        "expected one entry error for bad TLS-route inline CIDR; got: {:?}",
        errs
    );
}

#[test]
fn collect_inline_ip_entry_errors_empty_for_clean_config() {
    let esp = clean_esp();
    let errs = collect_inline_ip_entry_errors(&esp);
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
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
    let esp: EdgionStreamPlugins = serde_yaml::from_str(yaml).unwrap();
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
