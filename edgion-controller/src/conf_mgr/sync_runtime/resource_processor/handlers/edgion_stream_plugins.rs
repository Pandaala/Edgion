//! EdgionStreamPlugins Handler
//!
//! Handles EdgionStreamPlugins resources with Gateway API standard status management.

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, pick_resolved_refs_reason, update_condition, ResolvedRefsError,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_types, EdgionConfigDataStore, HandlerContext, ProcessResult, ProcessorHandler,
};
use edgion_resources::prelude_resources::{EdgionConfigData, EdgionStreamPlugins};
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataRef};
use edgion_resources::resources::edgion_plugins::PluginValidation;
use edgion_resources::resources::edgion_stream_plugins::{
    EdgionStreamPlugin, EdgionStreamPluginsStatus, TlsRouteStreamPlugin,
};

const SELF_KIND: &str = "EdgionStreamPlugins";

/// Prefix used to mark preparse errors that indicate a legacy-schema upgrade issue.
/// When present, `update_status` maps the condition reason to `LegacySchemaDeprecated`
/// (instead of the generic `Invalid`) so operators can grep status and find migration hints.
const LEGACY_SCHEMA_MARKER: &str = "[LEGACY_SCHEMA_DEPRECATED] ";

/// Condition reason emitted when an upgrade path collides with a breaking schema change.
const REASON_LEGACY_SCHEMA_DEPRECATED: &str = "LegacySchemaDeprecated";

/// Single source of truth for the migration doc path referenced from status conditions.
/// Keep this in sync with the actual file location; renaming the doc without updating this
/// constant leaves operators clicking a broken link.
const MIGRATION_DOC_PATH: &str = "docs/migrations/ec4e000-ip-restriction-stream-schema.md";

/// EdgionStreamPlugins handler
pub struct EdgionStreamPluginsHandler;

impl EdgionStreamPluginsHandler {
    pub fn new() -> Self {
        Self
    }

    fn self_ref_key(esp: &EdgionStreamPlugins) -> String {
        let ns = esp.metadata.namespace.as_deref().unwrap_or("");
        let name = esp.metadata.name.as_deref().unwrap_or("");
        format!("{}:{}/{}", SELF_KIND, ns, name)
    }

    /// Iterate each GlobalConnectionIpRestriction plugin that has an `activeProfileRef`
    /// and yield `(selector_namespace, selector_name)`.
    fn iter_gcir_refs<'a>(esp: &'a EdgionStreamPlugins) -> impl Iterator<Item = (String, String)> + 'a {
        let esp_ns = esp.metadata.namespace.as_deref().unwrap_or("").to_string();
        esp.spec
            .plugins
            .as_ref()
            .into_iter()
            .flat_map(|v| v.iter())
            .filter_map(move |entry| match &entry.plugin {
                EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                    // Only register a ref when activeProfileRef is set; the config is inline.
                    gcir.active_profile_ref.as_ref().map(|r| {
                        let ns = r.namespace.clone().unwrap_or_else(|| esp_ns.clone());
                        (ns, r.name.clone())
                    })
                }
                EdgionStreamPlugin::IpRestriction(_) => None,
                EdgionStreamPlugin::ConnectionRateLimit(_) => None,
            })
    }

    /// Collect `(namespace, name)` for every IpList referenced by `allow_refs`/`deny_refs` on
    /// the L4 `IpRestriction`, each GCIR profile, and the TLS-route `IpRestriction`. Namespace
    /// defaults to the ESP namespace. Returns a SORTED, de-duplicated Vec so the resulting
    /// status `message` is stable (GCIR `profiles` is a `HashMap` with unstable order).
    fn iter_ip_list_refs(esp: &EdgionStreamPlugins) -> Vec<(String, String)> {
        let esp_ns = esp.metadata.namespace.as_deref().unwrap_or("");
        let mut out: Vec<(String, String)> = Vec::new();

        // Stage 1: ConnectionFilter plugins.
        for entry in esp.spec.plugins.iter().flatten() {
            match &entry.plugin {
                EdgionStreamPlugin::IpRestriction(rules) => {
                    collect_ref_pairs(&mut out, esp_ns, rules.allow_refs.as_ref(), rules.deny_refs.as_ref());
                }
                EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                    for profile in gcir.profiles.values() {
                        collect_ref_pairs(
                            &mut out,
                            esp_ns,
                            profile.allow_refs.as_ref(),
                            profile.deny_refs.as_ref(),
                        );
                    }
                }
                EdgionStreamPlugin::ConnectionRateLimit(_) => {}
            }
        }

        // Stage 2: TlsRoute plugins (HTTP-layer `IpRestrictionConfig`).
        for entry in esp.spec.tls_route_plugins.iter().flatten() {
            match &entry.plugin {
                TlsRouteStreamPlugin::IpRestriction(config) => {
                    collect_ref_pairs(&mut out, esp_ns, config.allow_refs.as_ref(), config.deny_refs.as_ref());
                }
            }
        }

        out.sort();
        out.dedup();
        out
    }
}

impl Default for EdgionStreamPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionStreamPlugins> for EdgionStreamPluginsHandler {
    fn preparse(&self, esp: &mut EdgionStreamPlugins, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = Vec::new();

        // Both stages validate disabled entries with `is_enabled()` skipped, mirroring the
        // runtime builders (`from_stream_plugins` / `from_entries`) which never construct a
        // disabled plugin — a parked broken config must not flip `Accepted` to False.
        //
        // Validation routes through the stateless `PluginValidation::validation_error()`
        // trait, mirroring the HTTP-plugin preparse pattern. The structural checks match
        // what the data-plane plugins (`StreamIpRestriction::new`,
        // `TlsRouteIpRestriction::new`) would hit at construction. Both stage plugins fail
        // closed on an invalid config — every connection denied — but without surfacing
        // the error here the CR keeps a healthy-looking `Accepted=True`, i.e. a silent
        // blackhole. Emitting a non-legacy error makes `update_status` set
        // `Accepted=False` with the `Invalid` reason.

        // Stage 1: ConnectionFilter plugins (pre-TLS, inline L4 `ConnectionIpRestrictionRules`).
        for (idx, entry) in esp.spec.plugins.iter().flatten().enumerate() {
            if !entry.is_enabled() {
                continue;
            }
            if let EdgionStreamPlugin::IpRestriction(rules) = &entry.plugin {
                let legacy = rules.legacy_http_fields();
                if !legacy.is_empty() {
                    errors.push(format!(
                        "{}plugins[{}] (IpRestriction): legacy HTTP fields {:?} detected on L4 config; \
                         remove them and use only allow/deny/defaultAction (see {})",
                        LEGACY_SCHEMA_MARKER, idx, legacy, MIGRATION_DOC_PATH
                    ));
                    continue;
                }
                if let Some(e) = rules.validation_error() {
                    errors.push(format!("plugins[{idx}] (IpRestriction): {e}"));
                }
            }
            if let EdgionStreamPlugin::ConnectionRateLimit(cfg) = &entry.plugin {
                if let Some(e) = cfg.validation_error() {
                    errors.push(format!("plugins[{idx}] (ConnectionRateLimit): {e}"));
                }
            }
            // Validate GlobalConnectionIpRestriction inline config (profiles + active profile).
            if let EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) = &entry.plugin {
                let mut config = gcir.clone();
                if let Err(e) = config.validate_and_init() {
                    errors.push(format!(
                        "plugins[{idx}] (GlobalConnectionIpRestriction): config invalid: {e}"
                    ));
                }
            }
        }

        // Stage 2: TlsRoute plugins (post-handshake, HTTP-layer `IpRestrictionConfig`).
        for (idx, entry) in esp.spec.tls_route_plugins.iter().flatten().enumerate() {
            if !entry.is_enabled() {
                continue;
            }
            match &entry.plugin {
                TlsRouteStreamPlugin::IpRestriction(config) => {
                    if let Err(e) = config.clone().validate_and_init() {
                        errors.push(format!("tlsRoutePlugins[{idx}] (IpRestriction): {e}"));
                    }
                }
            }
        }

        errors
    }

    async fn parse(&self, esp: EdgionStreamPlugins, ctx: &HandlerContext) -> ProcessResult<EdgionStreamPlugins> {
        // Rebuild outgoing EdgionConfigData refs from scratch so that an edit that removes
        // a GCIR entry correctly drops its old ref.
        let self_ref = Self::self_ref_key(&esp);
        ctx.runtime().edgion_config_data_refs.clear_refs_from(&self_ref);
        for (ed_ns, ed_name) in Self::iter_gcir_refs(&esp) {
            let ed_key = EdgionConfigDataStore::make_key(Some(&ed_ns), &ed_name);
            ctx.runtime().edgion_config_data_refs.add_ref(ed_key, self_ref.clone());
        }
        for (ed_ns, ed_name) in Self::iter_ip_list_refs(&esp) {
            let ed_key = EdgionConfigDataStore::make_key(Some(&ed_ns), &ed_name);
            ctx.runtime().edgion_config_data_refs.add_ref(ed_key, self_ref.clone());
        }
        ProcessResult::Continue(esp)
    }

    async fn on_change(&self, esp: &EdgionStreamPlugins, ctx: &HandlerContext) {
        requeue_referencing_gateways(esp, ctx).await;
    }

    async fn on_delete(&self, esp: &EdgionStreamPlugins, ctx: &HandlerContext) {
        ctx.runtime()
            .edgion_config_data_refs
            .clear_refs_from(&Self::self_ref_key(esp));
        requeue_referencing_gateways(esp, ctx).await;
    }

    fn update_status(&self, esp: &mut EdgionStreamPlugins, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = esp.metadata.generation;

        // Accepted: preparse/validate errors (with legacy-schema reason mapping).
        let accepted = if validation_errors.is_empty() {
            condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Resource accepted",
                generation,
            )
        } else {
            let has_legacy = validation_errors.iter().any(|e| e.starts_with(LEGACY_SCHEMA_MARKER));
            let reason = if has_legacy {
                REASON_LEGACY_SCHEMA_DEPRECATED
            } else {
                condition_reasons::INVALID
            };
            let message = validation_errors
                .iter()
                .map(|e| e.strip_prefix(LEGACY_SCHEMA_MARKER).unwrap_or(e))
                .collect::<Vec<_>>()
                .join("; ");
            condition_false(condition_types::ACCEPTED, reason, message, generation)
        };

        // ResolvedRefs: each GlobalConnectionIpRestriction activeProfileRef must
        // resolve to an existing EdgionConfigData of the `Selector` variant.
        // IpList allow_refs/deny_refs (L4, GCIR-per-profile, TLS-route) are also checked.
        let mut resolved_errors = resolve_refs(esp, ctx);
        resolved_errors.extend(resolve_ip_list_refs(esp, ctx));
        let resolved_refs = if resolved_errors.is_empty() {
            condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "All references resolved",
                generation,
            )
        } else {
            let reason = pick_resolved_refs_reason(&resolved_errors);
            let message = resolved_errors
                .iter()
                .map(|e| e.message())
                .collect::<Vec<_>>()
                .join("; ");
            condition_false(condition_types::RESOLVED_REFS, reason, message, generation)
        };

        let status = esp
            .status
            .get_or_insert_with(|| EdgionStreamPluginsStatus { conditions: vec![] });
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
    }
}

/// Resolve each IpList `allow_refs`/`deny_refs` (L4, GCIR-per-profile, TLS-route) against the
/// EdgionConfigData store. One `EdgionConfigDataNotFound { expected_type: "IpList" }` per
/// missing or wrong-type reference. Order is deterministic (`iter_ip_list_refs` is sorted).
fn resolve_ip_list_refs(esp: &EdgionStreamPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for (ns, name) in EdgionStreamPluginsHandler::iter_ip_list_refs(esp) {
        let target: Option<EdgionConfigData> = ctx.runtime().edgion_config_data_store.get(Some(&ns), &name);
        let resolved = target.as_ref().is_some_and(|t| {
            t.spec.enable
                && matches!(&t.spec.data, ConfigEntry::IpList(d)
                    if edgion_resources::matcher::ip_cidr_helpers::build_grouped_matcher(&d.items).is_ok())
        });
        if !resolved {
            errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                namespace: ns,
                name,
                expected_type: "IpList".to_string(),
            });
        }
    }
    errors
}

/// Requeue all Gateways that reference this EdgionStreamPlugins via
/// `edgion.io/edgion-stream-plugins` annotation.
async fn requeue_referencing_gateways(esp: &EdgionStreamPlugins, ctx: &HandlerContext) {
    let Some(esp_name) = esp.metadata.name.as_deref().filter(|n| !n.is_empty()) else {
        return;
    };
    let esp_key = format!(
        "{}/{}",
        esp.metadata.namespace.as_deref().unwrap_or("default"),
        esp_name
    );
    let refs = ctx.runtime().stream_plugin_ref_mgr.get_refs(&esp_key);
    if !refs.is_empty() {
        tracing::info!(
            esp_key = %esp_key,
            ref_count = refs.len(),
            "EdgionStreamPlugins changed, requeuing referencing Gateways"
        );
    }
    for gw_ref in refs {
        ctx.requeue("Gateway", gw_ref.resource_key()).await;
    }
}

/// Push `(namespace, name)` for each ref in `allow`/`deny`, defaulting an unset ref namespace
/// to `esp_ns`.
fn collect_ref_pairs(
    out: &mut Vec<(String, String)>,
    esp_ns: &str,
    allow: Option<&Vec<EdgionConfigDataRef>>,
    deny: Option<&Vec<EdgionConfigDataRef>>,
) {
    for r in allow.into_iter().flatten().chain(deny.into_iter().flatten()) {
        let ns = r.namespace.clone().unwrap_or_else(|| esp_ns.to_string());
        out.push((ns, r.name.clone()));
    }
}

/// Resolve each GCIR `activeProfileRef` against the EdgionConfigData store.
/// Returns one error per unresolved or wrong-type reference.
/// Only entries that have `activeProfileRef` set are checked (the config is always inline).
fn resolve_refs(esp: &EdgionStreamPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for (ns, name) in EdgionStreamPluginsHandler::iter_gcir_refs(esp) {
        let target: Option<EdgionConfigData> = ctx.runtime().edgion_config_data_store.get(Some(&ns), &name);
        let resolved = target
            .as_ref()
            .is_some_and(|t| matches!(&t.spec.data, ConfigEntry::Selector(_)));
        if !resolved {
            errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                namespace: ns,
                name,
                expected_type: "Selector".to_string(),
            });
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::prelude_resources::EdgionStreamPlugins;
    use std::sync::Arc;

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
                enable: true,
                active: None,
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

    fn make_ctx() -> HandlerContext {
        HandlerContext::new(
            None,
            None,
            TriggerChain::default(),
            3,
            20,
            Arc::new(ControllerRuntime::default()),
        )
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
        ipSource: RemoteAddr
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
        let accepted = status.conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.reason, "Invalid");
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
                enable: false,
                active: None,
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
}
