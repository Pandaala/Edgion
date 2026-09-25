//! EdgionStreamPlugins Handler
//!
//! Handles EdgionStreamPlugins resources with Gateway API standard status management.

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, partially_invalid_condition, pick_resolved_refs_reason,
    update_condition, ResolvedRefsError,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_types, EdgionConfigDataStore, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::observe::unknown_fields_registry::bounded_unknown_field_paths;
use edgion_resources::ResourceKind;

use super::config_data_ref_mark::{mark_one_ref, ExpectedEntry};
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
                EdgionStreamPlugin::IpRestriction(_) | EdgionStreamPlugin::Unsupported => None,
                EdgionStreamPlugin::ConnectionRateLimit(_) => None,
                EdgionStreamPlugin::GeoIpLocation(_) => None,
            })
    }

    /// Set or clear `ref_denied` on every EdgionConfigDataRef in this EdgionStreamPlugins.
    ///
    /// Called in `parse()` after ref registration, before `ProcessResult::Continue`.
    /// Same-namespace refs are always cleared. Cross-namespace refs are marked denied
    /// when no matching ReferenceGrant permits them and the target exists with the
    /// expected ConfigEntry type. Missing / wrong-type targets are not marked here —
    /// those surface as `EdgionConfigDataNotFound` in `update_status`.
    fn mark_config_data_ref_denials(esp: &mut EdgionStreamPlugins, ctx: &HandlerContext) {
        let from_ns = esp.metadata.namespace.as_deref().unwrap_or("default").to_string();

        // Stage 1: connection-filter plugins (exhaustive over EdgionStreamPlugin).
        for entry in esp.spec.plugins.iter_mut().flatten() {
            match &mut entry.plugin {
                EdgionStreamPlugin::IpRestriction(rules) => {
                    for r in rules
                        .allow_refs
                        .iter_mut()
                        .flatten()
                        .chain(rules.deny_refs.iter_mut().flatten())
                    {
                        mark_one_ref(r, &from_ns, "EdgionStreamPlugins", ExpectedEntry::IpList, ctx);
                    }
                }
                EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                    if let Some(r) = gcir.active_profile_ref.as_mut() {
                        mark_one_ref(r, &from_ns, "EdgionStreamPlugins", ExpectedEntry::Selector, ctx);
                    }
                    for profile in gcir.profiles.values_mut() {
                        for r in profile
                            .allow_refs
                            .iter_mut()
                            .flatten()
                            .chain(profile.deny_refs.iter_mut().flatten())
                        {
                            mark_one_ref(r, &from_ns, "EdgionStreamPlugins", ExpectedEntry::IpList, ctx);
                        }
                    }
                }
                EdgionStreamPlugin::ConnectionRateLimit(_) | EdgionStreamPlugin::Unsupported => {}
                EdgionStreamPlugin::GeoIpLocation(_) => {}
            }
        }

        // Stage 2: TLS-route plugins (exhaustive over TlsRouteStreamPlugin).
        for entry in esp.spec.tls_route_plugins.iter_mut().flatten() {
            match &mut entry.plugin {
                TlsRouteStreamPlugin::Unsupported => {}
                TlsRouteStreamPlugin::IpRestriction(config) => {
                    for r in config
                        .allow_refs
                        .iter_mut()
                        .flatten()
                        .chain(config.deny_refs.iter_mut().flatten())
                    {
                        mark_one_ref(r, &from_ns, "EdgionStreamPlugins", ExpectedEntry::IpList, ctx);
                    }
                }
            }
        }
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
                EdgionStreamPlugin::ConnectionRateLimit(_) | EdgionStreamPlugin::Unsupported => {}
                EdgionStreamPlugin::GeoIpLocation(_) => {}
            }
        }

        // Stage 2: TlsRoute plugins (HTTP-layer `IpRestrictionConfig`).
        for entry in esp.spec.tls_route_plugins.iter().flatten() {
            match &entry.plugin {
                TlsRouteStreamPlugin::Unsupported => {}
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
        // Unknown-field diagnostic: `spec.unknown_fields` was already
        // populated by the ingest hook, loader, or Admin API seam, so this
        // is a pure read — no re-walk here. Warn once on the apply path; the
        // status-side UnknownConfigFields condition is stamped in
        // update_status. Advisory only:
        // this never touches `Accepted` or blocks the resource from syncing.
        if !esp.spec.unknown_fields.is_empty() {
            let paths = bounded_unknown_field_paths(&esp.spec.unknown_fields);
            tracing::warn!(
                component = "conf_mgr",
                event = "unknown_config_fields_detected",
                kind = "EdgionStreamPlugins",
                namespace = %esp.metadata.namespace.as_deref().unwrap_or("default"),
                name = %esp.metadata.name.as_deref().unwrap_or(""),
                paths = ?paths,
                "EdgionStreamPlugins document has keys the schema does not recognize; they are ignored, config still syncs unchanged"
            );
        }

        let mut errors = esp.unsupported_plugin_errors();

        // Known disabled entries are skipped, mirroring the
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
            // GeoIpLocation: stateless field validation, layer-parameterized
            // (`is_stream: true`). This does NOT go through the generic
            // `PluginValidation::validation_error()` trait (that fixed
            // signature has no layer flag) — it calls the exact same
            // `GeoIpLocationConfig::check_fields(true)` the gateway's
            // `GeoIpLocation` stream-plugin constructor calls via
            // `validate_and_init(true)`, so the two paths cannot drift. See
            // `GeoIpLocationConfig`'s module doc.
            if let EdgionStreamPlugin::GeoIpLocation(cfg) = &entry.plugin {
                if let Some(e) = cfg.check_fields(true) {
                    errors.push(format!("plugins[{idx}] (GeoIpLocation): {e}"));
                }
            }
        }

        // Stage 2: TlsRoute plugins (post-handshake, HTTP-layer `IpRestrictionConfig`).
        for (idx, entry) in esp.spec.tls_route_plugins.iter().flatten().enumerate() {
            if !entry.is_enabled() {
                continue;
            }
            match &entry.plugin {
                TlsRouteStreamPlugin::Unsupported => {}
                TlsRouteStreamPlugin::IpRestriction(config) => {
                    if let Err(e) = config.clone().validate_and_init() {
                        errors.push(format!("tlsRoutePlugins[{idx}] (IpRestriction): {e}"));
                    }
                }
            }
        }

        errors
    }

    async fn parse(&self, mut esp: EdgionStreamPlugins, ctx: &HandlerContext) -> ProcessResult<EdgionStreamPlugins> {
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

        // Grant-change revalidation: register cross-ns EdgionConfigData ref target namespaces
        // into the namespace-keyed cross_ns_ref_mgr (mirrors http_route::record_cross_ns_refs).
        // When a ReferenceGrant in a target namespace changes, the CrossNsRevalidationListener
        // requeues this ESP so ref_denied markers and status are recomputed.
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionStreamPlugins,
            esp.metadata.namespace.clone(),
            esp.metadata.name.clone().unwrap_or_default(),
        );
        let esp_ns = esp.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let cross_mgr = &ctx.runtime().cross_ns_ref_mgr;
        cross_mgr.clear_value_refs(&resource_ref);
        for (target_ns, _name) in Self::iter_gcir_refs(&esp).chain(Self::iter_ip_list_refs(&esp)) {
            if target_ns != esp_ns {
                cross_mgr.add_ref(target_ns, resource_ref.clone());
            }
        }

        // Mark cross-namespace EdgionConfigData refs as denied when no ReferenceGrant permits them.
        Self::mark_config_data_ref_denials(&mut esp, ctx);
        ProcessResult::Continue(esp)
    }

    async fn on_change(&self, esp: &EdgionStreamPlugins, ctx: &HandlerContext) {
        requeue_referencing_gateways(esp, ctx).await;
    }

    async fn on_delete(&self, esp: &EdgionStreamPlugins, ctx: &HandlerContext) {
        ctx.runtime()
            .edgion_config_data_refs
            .clear_refs_from(&Self::self_ref_key(esp));
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionStreamPlugins,
            esp.metadata.namespace.clone(),
            esp.metadata.name.clone().unwrap_or_default(),
        );
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
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

        let inline_errors = collect_inline_ip_entry_errors(esp);
        let partially_invalid = if inline_errors.is_empty() {
            partially_invalid_condition(None, generation)
        } else {
            let msg = edgion_resources::matcher::ip_cidr_helpers::format_entry_errors(&inline_errors);
            partially_invalid_condition(Some(msg), generation)
        };

        let Some(status) = esp
            .status
            .get_or_insert_with(EdgionStreamPluginsStatus::default)
            .as_local_mut()
        else {
            tracing::warn!(
                component = "edgion_stream_plugins_handler",
                "Status projection missing; skipping status mutation"
            );
            return;
        };
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
        update_condition(&mut status.conditions, partially_invalid);
        super::super::unknown_fields_stamp::apply_unknown_fields_condition(
            &mut status.conditions,
            &esp.spec.unknown_fields,
            generation,
        );
    }
}

/// Resolve each IpList `allow_refs`/`deny_refs` (L4, GCIR-per-profile, TLS-route) against the
/// EdgionConfigData store. Returns one `RefNotPermitted` error when `ref_denied` is set
/// (cross-namespace denial), or one `EdgionConfigDataNotFound { expected_type: "IpList" }` per
/// missing or wrong-type reference. Mutually exclusive per ref via else-branch to avoid a TOCTOU
/// double-message. GCIR profiles are iterated in sorted key order for deterministic message output.
fn resolve_ip_list_refs(esp: &EdgionStreamPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let from_ns = esp.metadata.namespace.as_deref().unwrap_or("default");
    let mut errors = Vec::new();

    // Stage 1: ConnectionFilter plugins.
    for entry in esp.spec.plugins.iter().flatten() {
        match &entry.plugin {
            EdgionStreamPlugin::IpRestriction(rules) => {
                for r in rules
                    .allow_refs
                    .iter()
                    .flatten()
                    .chain(rules.deny_refs.iter().flatten())
                {
                    let rns = r.namespace.as_deref().unwrap_or(from_ns);
                    if r.ref_denied.is_some() {
                        errors.push(ResolvedRefsError::RefNotPermitted {
                            from_namespace: from_ns.to_string(),
                            from_kind: "EdgionStreamPlugins",
                            target_namespace: rns.to_string(),
                            target_name: r.name.clone(),
                        });
                    } else {
                        let target: Option<EdgionConfigData> =
                            ctx.runtime().edgion_config_data_store.get(Some(rns), &r.name);
                        let resolved = target
                            .as_ref()
                            .is_some_and(|t| t.spec.enable && matches!(&t.spec.data, ConfigEntry::IpList(_)));
                        if !resolved {
                            errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                                namespace: rns.to_string(),
                                name: r.name.clone(),
                                expected_type: "IpList".to_string(),
                            });
                        }
                    }
                }
            }
            EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                // Iterate profiles in sorted key order for deterministic message output
                // (HashMap iteration order is not stable).
                let mut profile_names: Vec<&String> = gcir.profiles.keys().collect();
                profile_names.sort();
                for pname in profile_names {
                    if let Some(profile) = gcir.profiles.get(pname) {
                        for r in profile
                            .allow_refs
                            .iter()
                            .flatten()
                            .chain(profile.deny_refs.iter().flatten())
                        {
                            let rns = r.namespace.as_deref().unwrap_or(from_ns);
                            if r.ref_denied.is_some() {
                                errors.push(ResolvedRefsError::RefNotPermitted {
                                    from_namespace: from_ns.to_string(),
                                    from_kind: "EdgionStreamPlugins",
                                    target_namespace: rns.to_string(),
                                    target_name: r.name.clone(),
                                });
                            } else {
                                let target: Option<EdgionConfigData> =
                                    ctx.runtime().edgion_config_data_store.get(Some(rns), &r.name);
                                let resolved = target
                                    .as_ref()
                                    .is_some_and(|t| t.spec.enable && matches!(&t.spec.data, ConfigEntry::IpList(_)));
                                if !resolved {
                                    errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                                        namespace: rns.to_string(),
                                        name: r.name.clone(),
                                        expected_type: "IpList".to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            EdgionStreamPlugin::ConnectionRateLimit(_) | EdgionStreamPlugin::Unsupported => {}
            EdgionStreamPlugin::GeoIpLocation(_) => {}
        }
    }

    // Stage 2: TLS-route plugins.
    for entry in esp.spec.tls_route_plugins.iter().flatten() {
        match &entry.plugin {
            TlsRouteStreamPlugin::Unsupported => {}
            TlsRouteStreamPlugin::IpRestriction(config) => {
                for r in config
                    .allow_refs
                    .iter()
                    .flatten()
                    .chain(config.deny_refs.iter().flatten())
                {
                    let rns = r.namespace.as_deref().unwrap_or(from_ns);
                    if r.ref_denied.is_some() {
                        errors.push(ResolvedRefsError::RefNotPermitted {
                            from_namespace: from_ns.to_string(),
                            from_kind: "EdgionStreamPlugins",
                            target_namespace: rns.to_string(),
                            target_name: r.name.clone(),
                        });
                    } else {
                        let target: Option<EdgionConfigData> =
                            ctx.runtime().edgion_config_data_store.get(Some(rns), &r.name);
                        let resolved = target
                            .as_ref()
                            .is_some_and(|t| t.spec.enable && matches!(&t.spec.data, ConfigEntry::IpList(_)));
                        if !resolved {
                            errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                                namespace: rns.to_string(),
                                name: r.name.clone(),
                                expected_type: "IpList".to_string(),
                            });
                        }
                    }
                }
            }
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

/// Collect partial-good entry errors over every inline allow/deny in L4 IpRestriction
/// (spec.plugins), each GCIR ProfileRules (gcir.profiles), and TLS-route IpRestriction
/// (spec.tls_route_plugins). GCIR present-but-empty is a separate structural concern
/// (Accepted), not reported here.
fn collect_inline_ip_entry_errors(
    esp: &EdgionStreamPlugins,
) -> Vec<edgion_resources::matcher::ip_cidr_helpers::EntryError> {
    use edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors;
    let mut errs = Vec::new();
    for entry in esp.spec.plugins.iter().flatten() {
        if let EdgionStreamPlugin::IpRestriction(rules) = &entry.plugin {
            if let Some(g) = &rules.allow {
                errs.extend(collect_entry_errors(g));
            }
            if let Some(g) = &rules.deny {
                errs.extend(collect_entry_errors(g));
            }
        }
        if let EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) = &entry.plugin {
            for profile in gcir.profiles.values() {
                if let Some(g) = &profile.allow {
                    errs.extend(collect_entry_errors(g));
                }
                if let Some(g) = &profile.deny {
                    errs.extend(collect_entry_errors(g));
                }
            }
        }
    }
    for entry in esp.spec.tls_route_plugins.iter().flatten() {
        let config = match &entry.plugin {
            TlsRouteStreamPlugin::IpRestriction(config) => config,
            TlsRouteStreamPlugin::Unsupported => continue,
        };
        if let Some(g) = &config.allow {
            errs.extend(collect_entry_errors(g));
        }
        if let Some(g) = &config.deny {
            errs.extend(collect_entry_errors(g));
        }
    }
    errs
}

/// Resolve each GCIR `activeProfileRef` against the EdgionConfigData store.
/// Returns one `RefNotPermitted` error when `ref_denied` is set (cross-namespace denial),
/// or one `EdgionConfigDataNotFound { expected_type: "Selector" }` per unresolved or wrong-type
/// reference. Mutually exclusive per ref via else-branch to avoid a TOCTOU double-message.
/// Iterates the real `&EdgionConfigDataRef` objects (not `(ns,name)` pairs) to preserve the
/// `ref_denied` marker set by `parse()`.
fn resolve_refs(esp: &EdgionStreamPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let from_ns = esp.metadata.namespace.as_deref().unwrap_or("default");
    let mut errors = Vec::new();
    for entry in esp.spec.plugins.iter().flatten() {
        if let EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) = &entry.plugin {
            if let Some(r) = &gcir.active_profile_ref {
                let rns = r.namespace.as_deref().unwrap_or(from_ns);
                if r.ref_denied.is_some() {
                    errors.push(ResolvedRefsError::RefNotPermitted {
                        from_namespace: from_ns.to_string(),
                        from_kind: "EdgionStreamPlugins",
                        target_namespace: rns.to_string(),
                        target_name: r.name.clone(),
                    });
                } else {
                    let target: Option<EdgionConfigData> =
                        ctx.runtime().edgion_config_data_store.get(Some(rns), &r.name);
                    let resolved = target
                        .as_ref()
                        .is_some_and(|t| t.spec.enable && matches!(&t.spec.data, ConfigEntry::Selector(_)));
                    if !resolved {
                        errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                            namespace: rns.to_string(),
                            name: r.name.clone(),
                            expected_type: "Selector".to_string(),
                        });
                    }
                }
            }
        }
    }
    errors
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins_test.rs"]
mod tests;
