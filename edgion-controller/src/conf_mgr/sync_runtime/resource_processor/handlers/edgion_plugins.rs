//! EdgionPlugins Handler
//!
//! Handles EdgionPlugins resources with Gateway API standard status management.
//! Delegates per-plugin Secret resolution to `plugins::resolve_credential`.
//!
//! Features:
//! - parse: Resolve Secret references and register to SecretRefManager for cascading updates
//! - parse: Register every EdgionConfigData dependency for cascading requeue
//! - parse: Expand RequestAccessPolicy IP/URL data and then bind its Secret/HMAC conditions
//! - on_delete: Clear SecretRefManager references + EdgionConfigData refs
//! - update_status: Validate `overrideRef` resolves to a `RegionRouteOverride` EdgionConfigData

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, partially_invalid_condition, pick_resolved_refs_reason,
    update_condition, ResolvedRefsError,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_types, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef, CONFIG_MAP_REF_KEY_PREFIX,
};
use crate::observe::unknown_fields_registry::bounded_unknown_field_paths;
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::{
    EdgionPlugin, EdgionPluginsStatus, PluginValidation, ResolvedWafBundle, WafBundleResolutionError,
    WafBundleResolutionKind,
};
use edgion_resources::ResourceKind;

use super::config_data_ref_mark::{mark_one_ref, ExpectedEntry};
use super::plugins::resolve_credential as cred;
use super::san_guard;

pub struct EdgionPluginsHandler;

impl EdgionPluginsHandler {
    fn get_waf_policy_ref_key(ep: &EdgionPlugins) -> Option<String> {
        let namespace = ep.metadata.namespace.as_deref().unwrap_or("default");
        ep.spec
            .waf
            .as_ref()
            .filter(|waf| waf.validate_shape().is_ok())
            .map(|waf| {
                format!(
                    "{}/{}",
                    waf.policy_ref.namespace.as_deref().unwrap_or(namespace),
                    waf.policy_ref.name
                )
            })
    }

    fn get_waf_selector_ref_key(ep: &EdgionPlugins) -> Option<String> {
        let namespace = ep.metadata.namespace.as_deref().unwrap_or("default");
        ep.spec
            .waf
            .as_ref()
            .filter(|waf| waf.validate_shape().is_ok())
            .and_then(|waf| {
                waf.active_profile_ref.as_ref().map(|reference| {
                    format!(
                        "{}/{}",
                        reference.namespace.as_deref().unwrap_or(namespace),
                        reference.name
                    )
                })
            })
    }

    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// RegionRoute overrideRef helpers
// ============================================================================

impl EdgionPluginsHandler {
    /// Clear all EdgionConfigData reverse refs registered by this EdgionPlugins.
    fn clear_config_data_refs(ep: &EdgionPlugins, ctx: &HandlerContext) {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let ep_name = ep.metadata.name.as_deref().unwrap_or("");
        let ep_key = format!("{}:{}/{}", ResourceKind::EdgionPlugins.as_str(), ns, ep_name);
        ctx.runtime().edgion_config_data_refs.clear_refs_from(&ep_key);
    }

    /// Collect all `overrideRef` EdgionConfigData keys from RegionRoute plugins.
    /// Returns "namespace/name" keys (namespace defaults to EP's namespace when absent).
    fn get_region_route_override_keys(ep: &EdgionPlugins) -> Vec<String> {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let Some(plugins) = &ep.spec.request_plugins else {
            return vec![];
        };
        plugins
            .iter()
            .filter_map(|e| {
                if let EdgionPlugin::RegionRoute(ref config) = e.plugin {
                    config.override_ref.as_ref().map(|r| {
                        let override_ns = r.namespace.as_deref().unwrap_or(ns);
                        format!("{}/{}", override_ns, r.name)
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Collect "namespace/name" keys for every IpList referenced by an `IpRestriction`
    /// plugin's `allow_refs`/`deny_refs` and by operator-authored Condition `ipRefs`
    /// (entry-level / RR / body / dye). Namespace defaults to the EP namespace.
    fn get_ip_list_ref_keys(ep: &EdgionPlugins) -> Vec<String> {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let mut keys = Vec::new();
        if let Some(plugins) = &ep.spec.request_plugins {
            for e in plugins {
                if let EdgionPlugin::IpRestriction(config) = &e.plugin {
                    for r in config
                        .allow_refs
                        .iter()
                        .flatten()
                        .chain(config.deny_refs.iter().flatten())
                    {
                        let rns = r.namespace.as_deref().unwrap_or(ns);
                        keys.push(format!("{}/{}", rns, r.name));
                    }
                }
            }
        }
        super::plugins::condition_tree::for_each_fail_closed_ip_ref(ep, |r| {
            let rns = r.namespace.as_deref().unwrap_or(ns);
            keys.push(format!("{}/{}", rns, r.name));
        });
        keys
    }

    /// Collect "namespace/name" keys for every `RequestAccessPolicy` plugin's
    /// `active_profile_ref` in `request_plugins`. Namespace defaults to the EP namespace.
    /// Mirrors `get_region_route_override_keys` for the policy ref site.
    fn get_active_profile_ref_keys(ep: &EdgionPlugins) -> Vec<String> {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let Some(plugins) = &ep.spec.request_plugins else {
            return vec![];
        };
        plugins
            .iter()
            .filter_map(|e| {
                if let EdgionPlugin::RequestAccessPolicy(ref config) = e.plugin {
                    config.active_profile_ref.as_ref().map(|r| {
                        let ref_ns = r.namespace.as_deref().unwrap_or(ns);
                        format!("{}/{}", ref_ns, r.name)
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Collect "namespace/name" keys for every `Canary` plugin's
    /// `active_profile_ref` in `request_plugins`. Namespace defaults to the
    /// EP namespace. Mirrors `get_active_profile_ref_keys` for the Canary ref site.
    fn get_canary_active_profile_ref_keys(ep: &EdgionPlugins) -> Vec<String> {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let Some(plugins) = &ep.spec.request_plugins else {
            return vec![];
        };
        plugins
            .iter()
            .filter_map(|e| {
                if let EdgionPlugin::Canary(ref config) = e.plugin {
                    config.active_profile_ref.as_ref().map(|r| {
                        let ref_ns = r.namespace.as_deref().unwrap_or(ns);
                        format!("{}/{}", ref_ns, r.name)
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Set or clear `ref_denied` on every EdgionConfigDataRef in this EdgionPlugins.
    ///
    /// Called in `parse()` after ref registration, before `ProcessResult::Continue`.
    /// Same-namespace refs are always cleared. Cross-namespace refs are marked denied
    /// when no matching ReferenceGrant permits them and the target exists with the
    /// expected ConfigEntry type. Missing / wrong-type targets are not marked here —
    /// those surface as `EdgionConfigDataNotFound` in `update_status`.
    fn mark_config_data_ref_denials(ep: &mut EdgionPlugins, ctx: &HandlerContext) {
        let from_ns = ep.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let Some(plugins) = ep.spec.request_plugins.as_mut() else {
            return;
        };
        for entry in plugins.iter_mut() {
            match &mut entry.plugin {
                EdgionPlugin::RegionRoute(config) => {
                    if let Some(r) = config.override_ref.as_mut() {
                        mark_one_ref(r, &from_ns, "EdgionPlugins", ExpectedEntry::RegionRouteOverride, ctx);
                    }
                }
                EdgionPlugin::IpRestriction(config) => {
                    for r in config
                        .allow_refs
                        .iter_mut()
                        .flatten()
                        .chain(config.deny_refs.iter_mut().flatten())
                    {
                        mark_one_ref(r, &from_ns, "EdgionPlugins", ExpectedEntry::IpList, ctx);
                    }
                }
                EdgionPlugin::RequestAccessPolicy(config) => {
                    if let Some(r) = config.active_profile_ref.as_mut() {
                        mark_one_ref(r, &from_ns, "EdgionPlugins", ExpectedEntry::Selector, ctx);
                    }
                }
                EdgionPlugin::Canary(config) => {
                    if let Some(r) = config.active_profile_ref.as_mut() {
                        mark_one_ref(r, &from_ns, "EdgionPlugins", ExpectedEntry::Selector, ctx);
                    }
                }
                // Non-ref-bearing variants: explicit arms so a future ref-bearing
                // variant forces a compile error here (fail-safe completeness guard).
                EdgionPlugin::RequestHeaderModifier(_)
                | EdgionPlugin::AiProxy(_)
                | EdgionPlugin::ResponseHeaderModifier(_)
                | EdgionPlugin::RequestRedirect(_)
                | EdgionPlugin::UrlRewrite(_)
                | EdgionPlugin::RequestMirror(_)
                | EdgionPlugin::BasicAuth(_)
                | EdgionPlugin::CredentialInjector(_)
                | EdgionPlugin::Cors(_)
                | EdgionPlugin::Csrf(_)
                | EdgionPlugin::JwtAuth(_)
                | EdgionPlugin::JweDecrypt(_)
                | EdgionPlugin::HmacAuth(_)
                | EdgionPlugin::HeaderCertAuth(_)
                | EdgionPlugin::KeyAuth(_)
                | EdgionPlugin::LdapAuth(_)
                | EdgionPlugin::Mock(_)
                | EdgionPlugin::FaultInjection(_)
                | EdgionPlugin::DebugAccessLogToHeader(_)
                | EdgionPlugin::ProxyRewrite(_)
                | EdgionPlugin::RequestBodyBuffer(_)
                | EdgionPlugin::JsonSchemaValidation(_)
                | EdgionPlugin::FormJsonTransform(_)
                | EdgionPlugin::RequestRestriction(_)
                | EdgionPlugin::ResponseRewrite(_)
                | EdgionPlugin::RateLimitLocal(_)
                | EdgionPlugin::RateLimitRedis(_)
                | EdgionPlugin::CtxSet(_)
                | EdgionPlugin::WebhookKeyGet(_)
                | EdgionPlugin::RealIp(_)
                | EdgionPlugin::ForwardAuth(_)
                | EdgionPlugin::OpenidConnect(_)
                | EdgionPlugin::BandwidthLimit(_)
                | EdgionPlugin::DirectEndpoint(_)
                | EdgionPlugin::AllEndpointStatus(_)
                | EdgionPlugin::DynamicInternalUpstream(_)
                | EdgionPlugin::DynamicExternalUpstream(_)
                | EdgionPlugin::Dsl(_)
                | EdgionPlugin::RequestId(_)
                | EdgionPlugin::ExtProc(_)
                | EdgionPlugin::Guardrail(_)
                | EdgionPlugin::AiGuard(_)
                | EdgionPlugin::GeoIpLocation(_)
                | EdgionPlugin::Wasm(_) => {}
            }
        }
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionPlugins> for EdgionPluginsHandler {
    fn preparse(&self, ep: &mut EdgionPlugins, _ctx: &HandlerContext) -> Vec<String> {
        // The plugin runtime is built gateway-side (`compile_plugins`); the
        // schema carries no runtime. The controller only needs the validation
        // errors for the K8s status condition, so it runs the schema-layer
        // `validate_plugins` instead of compiling a runtime it would throw away.
        // This keeps the controller free of any `edgion-gateway` dependency.
        // DSL scripts are fully statically validated here too (compile /
        // bytecode / linkSysRefs, via the shared `edgion-dsl` crate); the only
        // residual gateway-only DSL failure is VM init, for which the data
        // plane still fails closed at runtime. See
        // `edgion_resources::resources::edgion_plugins::validate`.
        //
        // Unknown-field diagnostic: `spec.unknown_fields` was already
        // populated by the ingest hook, loader, or Admin API seam, so this
        // is a pure read — no re-walk here. Warn once on the apply path; the
        // status-side UnknownConfigFields condition is stamped in
        // update_status (which has no preparse-local state). Advisory only: this never
        // touches `Accepted` or blocks the resource from syncing.
        if !ep.spec.unknown_fields.is_empty() {
            let paths = bounded_unknown_field_paths(&ep.spec.unknown_fields);
            tracing::warn!(
                component = "conf_mgr",
                event = "unknown_config_fields_detected",
                kind = "EdgionPlugins",
                namespace = %ep.metadata.namespace.as_deref().unwrap_or("default"),
                name = %ep.metadata.name.as_deref().unwrap_or(""),
                paths = ?paths,
                "EdgionPlugins document has keys the schema does not recognize; they are ignored, config still syncs unchanged"
            );
        }

        if let Some(waf) = ep.spec.waf.as_mut() {
            waf.policy_ref.ref_denied = None;
            if let Some(reference) = waf.active_profile_ref.as_mut() {
                reference.ref_denied = None;
            }
            waf.resolved_policy = None;
            waf.selected_profile = None;
            waf.resolved_ref_indices.clear();
            waf.resolved_bundles = None;
            waf.resolution_errors.clear();
            waf.resolution_warnings.clear();
            waf.selector_resolved = None;
            waf.owner_namespace = None;
        }
        let mut errors = ep.validate_plugins_before_waf_resolution();
        if let Some(waf) = ep.spec.waf.as_ref() {
            if let Err(error) = waf.validate_shape() {
                errors.push(format!("waf: {error}"));
            }
        }
        // Phase 1 guard: reject outbound TLS subjectAltNames/hostname on plugins
        // where SAN pinning enforcement is not yet implemented.
        errors.extend(san_guard::check_plugins_san(ep));
        errors
    }

    async fn parse(&self, mut ep: EdgionPlugins, ctx: &HandlerContext) -> ProcessResult<EdgionPlugins> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );

        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        ctx.runtime().link_sys_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().service_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime()
            .credential_source_registry
            .remove_consumer(&resource_ref.resource_key());
        // One cross-namespace index covers both Secret and EdgionConfigData
        // references. Clear it before any resolver starts rebuilding either
        // class; clearing later would erase Secret refs registered above the
        // EdgionConfigData pass and prevent ReferenceGrant revalidation.
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);

        cred::resolve_basic_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_credential_injector(&mut ep, &resource_ref, ctx).await;
        cred::resolve_jwt(&mut ep, &resource_ref, ctx);
        cred::resolve_jwe(&mut ep, &resource_ref, ctx);
        cred::resolve_header_cert_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_hmac(&mut ep, &resource_ref, ctx);
        cred::resolve_key_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_openid_connect(&mut ep, &resource_ref, ctx);
        cred::resolve_ext_proc(&mut ep, &resource_ref, ctx);
        cred::resolve_ldap_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_csrf(&mut ep, &resource_ref, ctx);
        cred::resolve_forward_auth(&mut ep, &resource_ref, ctx).await;
        cred::resolve_guardrail(&mut ep, &resource_ref, ctx).await;
        cred::resolve_ai_guard(&mut ep, &resource_ref, ctx).await;
        cred::resolve_wasm(&mut ep, &resource_ref, ctx);

        // Clear existing EdgionConfigData refs for this EP, then re-register from scratch.
        Self::clear_config_data_refs(&ep, ctx);

        let ns = ep.metadata.namespace.as_deref().unwrap_or("default").to_string();
        let ep_name = ep.metadata.name.as_deref().unwrap_or("").to_string();
        let ep_key = format!("{}:{}/{}", ResourceKind::EdgionPlugins.as_str(), ns, ep_name);

        // Register requeue watcher: EP → EdgionConfigData(overrideRef).
        // When the referenced EdgionConfigData changes, the EdgionConfigDataHandler requeues this EP.
        for override_key in Self::get_region_route_override_keys(&ep) {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(override_key, ep_key.clone());
        }

        // Register requeue watcher: EP → EdgionConfigData(IpList refs).
        // When the referenced EdgionConfigData changes, the EdgionConfigDataHandler requeues this EP.
        for ip_list_key in Self::get_ip_list_ref_keys(&ep) {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(ip_list_key, ep_key.clone());
        }

        // Register requeue watcher: EP → EdgionConfigData(RequestAccessPolicy activeProfileRef).
        // Closes a pre-existing gap: active_profile_ref was not tracked before Task 6.
        for policy_key in Self::get_active_profile_ref_keys(&ep) {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(policy_key, ep_key.clone());
        }

        // Register requeue watcher: EP → EdgionConfigData(Canary activeProfileRef).
        for canary_key in Self::get_canary_active_profile_ref_keys(&ep) {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(canary_key, ep_key.clone());
        }

        if let Some(policy_key) = Self::get_waf_policy_ref_key(&ep) {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(policy_key, ep_key.clone());
        }
        if let Some(selector_key) = Self::get_waf_selector_ref_key(&ep) {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(selector_key, ep_key.clone());
        }

        // Grant-change revalidation: register cross-ns EdgionConfigData ref target namespaces
        // into the namespace-keyed cross_ns_ref_mgr (mirrors http_route::record_cross_ns_refs).
        // When a ReferenceGrant in a target namespace changes, the CrossNsRevalidationListener
        // requeues this EP so ref_denied markers and status are recomputed.
        let cross_mgr = &ctx.runtime().cross_ns_ref_mgr;
        for key in Self::get_region_route_override_keys(&ep)
            .into_iter()
            .chain(Self::get_ip_list_ref_keys(&ep))
            .chain(Self::get_active_profile_ref_keys(&ep))
            .chain(Self::get_canary_active_profile_ref_keys(&ep))
            .chain(Self::get_waf_policy_ref_key(&ep))
            .chain(Self::get_waf_selector_ref_key(&ep))
        {
            if let Some((target_ns, _)) = key.split_once('/') {
                if target_ns != ns {
                    cross_mgr.add_ref(target_ns.to_string(), resource_ref.clone());
                }
            }
        }

        // Mark cross-namespace EdgionConfigData refs as denied when no ReferenceGrant permits them.
        Self::mark_config_data_ref_denials(&mut ep, ctx);
        resolve_waf_bundles(&mut ep, ctx);

        for entry in ep.spec.request_plugins.iter_mut().flatten() {
            if let EdgionPlugin::RequestMirror(config) = &mut entry.plugin {
                super::request_mirror::resolve(&mut config.filter.backend_ref, &ns, |backend| {
                    // Subscribe before reading the grant so concurrent revocation
                    // either changes this read or requeues this resource.
                    cross_mgr.add_ref(
                        backend.namespace.as_deref().unwrap_or(&ns).to_string(),
                        resource_ref.clone(),
                    );
                    super::request_mirror::allowed(backend, &ns, ResourceKind::EdgionPlugins, ctx.runtime())
                });
            }
        }
        super::request_mirror::register_service_refs(
            &resource_ref,
            &ns,
            ep.spec
                .request_plugins
                .iter()
                .flatten()
                .filter(|entry| entry.is_enabled())
                .filter_map(|entry| {
                    if let EdgionPlugin::RequestMirror(config) = &entry.plugin {
                        Some(&config.filter.backend_ref)
                    } else {
                        None
                    }
                }),
            ctx.runtime(),
        );

        // Two ordered passes over every Condition tree: ConfigData expansion
        // (resolved_profiles / resolved_candidates / ipRefs) then credentials
        // (secretRef / HMAC). Expansion must run first so HMAC secretGroups
        // that exist only on imported URL candidates are visible.
        super::plugins::condition_tree::resolve(&mut ep, &resource_ref, ctx);
        ProcessResult::Continue(ep)
    }

    async fn on_change(&self, ep: &EdgionPlugins, ctx: &HandlerContext) {
        requeue_referencing_resources(ep, ctx).await;
    }

    async fn on_delete(&self, ep: &EdgionPlugins, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        ctx.runtime().link_sys_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().service_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime()
            .credential_source_registry
            .forget_consumer(&resource_ref.resource_key());
        Self::clear_config_data_refs(ep, ctx);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        requeue_referencing_resources(ep, ctx).await;

        tracing::debug!(
            edgion_plugins = %resource_ref.key(),
            "Cleared secret, LinkSys, EdgionConfigData, and cross-ns references on EdgionPlugins delete"
        );
    }

    fn update_status(&self, ep: &mut EdgionPlugins, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ep.metadata.generation;
        let mut validation_errors = validation_errors.to_vec();
        if let Some(waf) = ep.spec.waf.as_ref() {
            if waf.validate_shape().is_ok() {
                if let Some(error) = waf.validation_error() {
                    validation_errors.push(format!("waf: {error}"));
                }
            }
        }

        // Accepted reflects preparse + validate errors (already merged by processor).
        let candidate_identity = ep.spec.waf.as_ref().and_then(|waf| {
            let name = waf.selected_profile.as_deref()?;
            let bundles = waf.resolved_bundles.as_deref().unwrap_or_default();
            let identities = bundles
                .iter()
                .take(16)
                .map(|bundle| {
                    format!(
                        "{}@{}:{}",
                        bundle.identity,
                        bundle.data.version.chars().take(64).collect::<String>(),
                        bundle.data.profile.chars().take(64).collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            Some(format!("; WAF candidateProfile={name} bundles={identities}"))
        });
        let accepted = if validation_errors.is_empty() {
            let mut message = "Resource accepted".to_string();
            if let Some(identity) = candidate_identity.as_deref() {
                message.push_str(identity);
            }
            condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                message,
                generation,
            )
        } else {
            let mut message = validation_errors.join("; ");
            if let Some(identity) = candidate_identity.as_deref() {
                message.push_str(identity);
            }
            condition_false(
                condition_types::ACCEPTED,
                condition_reasons::INVALID,
                message,
                generation,
            )
        };

        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );
        let mut resolved_errors = Self::resolve_secret_refs(ep, ctx);
        resolved_errors.extend(cred::credential_injector_status_errors(ep, &resource_ref, ctx));
        resolved_errors.extend(cred::jwe_status_errors(ep));
        resolved_errors.extend(resolve_secret_match_refs(ep, ctx));
        resolved_errors.extend(resolve_guardrail_request_secret_refs(ep, ctx));
        resolved_errors.extend(resolve_wasm_refs(ep, ctx));
        resolved_errors.extend(resolve_override_refs(ep, ctx));
        resolved_errors.extend(resolve_ip_list_refs(ep, ctx));
        resolved_errors.extend(resolve_active_profile_refs(ep, ctx));
        resolved_errors.extend(resolve_canary_active_profile_refs(ep, ctx));
        resolved_errors.extend(resolve_waf_policy_ref(ep));
        resolved_errors.extend(resolve_waf_active_profile_ref(ep));
        if let Some(waf) = ep.spec.waf.as_ref() {
            for error in &waf.resolution_errors {
                resolved_errors.push(match error.kind {
                    WafBundleResolutionKind::Denied => ResolvedRefsError::RefNotPermitted {
                        from_namespace: waf.owner_namespace.clone().unwrap_or_else(|| "default".to_string()),
                        from_kind: "EdgionConfigData",
                        target_namespace: error.namespace.clone(),
                        target_name: error.name.clone(),
                    },
                    WafBundleResolutionKind::Missing | WafBundleResolutionKind::WrongType => {
                        ResolvedRefsError::EdgionConfigDataNotFound {
                            namespace: error.namespace.clone(),
                            name: error.name.clone(),
                            expected_type: "WafRuleBundle".to_string(),
                        }
                    }
                    WafBundleResolutionKind::Disabled
                    | WafBundleResolutionKind::Invalid
                    | WafBundleResolutionKind::DigestMismatch => ResolvedRefsError::EdgionConfigDataInvalid {
                        namespace: error.namespace.clone(),
                        name: error.name.clone(),
                        detail: format!(
                            "WafRuleBundle profile {} reference {} is {:?}",
                            error.profile, error.index, error.kind
                        ),
                    },
                });
            }
        }
        resolved_errors.extend(super::request_mirror::errors(
            ep.metadata.namespace.as_deref().unwrap_or("default"),
            ResourceKind::EdgionPlugins,
            ep.spec
                .request_plugins
                .iter()
                .flatten()
                .filter(|entry| entry.is_enabled())
                .filter_map(|entry| {
                    if let EdgionPlugin::RequestMirror(config) = &entry.plugin {
                        Some(&config.filter.backend_ref)
                    } else {
                        None
                    }
                }),
            ctx.runtime(),
        ));
        dedup_resolved_ref_errors(&mut resolved_errors);
        let resolved_refs = if resolved_errors.is_empty() {
            condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "All references resolved",
                generation,
            )
        } else {
            // Use the shared priority helper rather than `errors[0]` —
            // `SecretRefManager::get_dependencies` iterates a set with no
            // stable order, so taking the first element would make the
            // surfaced reason depend on hashing. `pick_resolved_refs_reason`
            // deterministically picks the most severe variant.
            let reason = pick_resolved_refs_reason(&resolved_errors);
            let message = resolved_errors
                .iter()
                .map(|e| e.message())
                .collect::<Vec<_>>()
                .join("; ");
            condition_false(condition_types::RESOLVED_REFS, reason, message, generation)
        };

        let inline_errors = collect_inline_ip_entry_errors(ep);
        let mut partial_messages = Vec::new();
        if !inline_errors.is_empty() {
            partial_messages.push(edgion_resources::matcher::ip_cidr_helpers::format_entry_errors(
                &inline_errors,
            ));
        }
        partial_messages.extend(request_access_policy_resolution_warnings(ep));
        if let Some(waf) = ep.spec.waf.as_ref() {
            partial_messages.extend(waf.resolution_warnings.iter().cloned());
        }
        let partially_invalid = if partial_messages.is_empty() {
            partially_invalid_condition(None, generation)
        } else {
            partially_invalid_condition(Some(partial_messages.join("; ")), generation)
        };

        let Some(status) = ep
            .status
            .get_or_insert_with(EdgionPluginsStatus::default)
            .as_local_mut()
        else {
            tracing::warn!(
                component = "edgion_plugins_handler",
                "Status projection missing; skipping status mutation"
            );
            return;
        };
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
        update_condition(&mut status.conditions, partially_invalid);
        super::super::unknown_fields_stamp::apply_unknown_fields_condition(
            &mut status.conditions,
            &ep.spec.unknown_fields,
            generation,
        );
    }
}

/// Re-run SecretMatch's selected-key usability checks without mutating the
/// dependency index. Missing objects and grant denials are already reported by
/// the generic dependency classifier; this adds present-but-unusable content.
fn resolve_secret_match_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::fetch_secret_readonly;
    use crate::conf_mgr::sync_runtime::resource_processor::credential::FetchOutcome;

    let owner = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        ep.metadata.namespace.clone(),
        ep.metadata.name.clone().unwrap_or_default(),
    );
    let owner_namespace = ep.metadata.namespace.as_deref().unwrap_or("default");
    let mut errors = Vec::new();
    super::plugins::condition_tree::for_each_secret_match(ep, |condition| {
        let FetchOutcome::Ok(secret) =
            fetch_secret_readonly(ctx, &owner, "edgion.io", "EdgionPlugins", &condition.secret_ref)
        else {
            return;
        };
        let namespace = condition.secret_ref.effective_namespace(owner_namespace).to_string();
        let Some(value) = secret.data.as_ref().and_then(|data| data.get(&condition.secret_key)) else {
            errors.push(ResolvedRefsError::SecretMatchKeyMissing {
                namespace,
                name: condition.secret_ref.name.clone(),
                key: condition.secret_key.clone(),
            });
            return;
        };
        match std::str::from_utf8(&value.0) {
            Err(_) => errors.push(ResolvedRefsError::SecretMatchValueInvalid {
                namespace,
                name: condition.secret_ref.name.clone(),
                key: condition.secret_key.clone(),
                detail: "value is not valid UTF-8",
            }),
            Ok(value) if value.trim().is_empty() => {
                errors.push(ResolvedRefsError::SecretMatchValueInvalid {
                    namespace,
                    name: condition.secret_ref.name.clone(),
                    key: condition.secret_key.clone(),
                    detail: "value is empty or whitespace-only",
                });
            }
            Ok(_) => {}
        }
    });
    errors
}

/// Re-evaluate Guardrail request-template Secret refs for the full
/// Gateway-API status classification. Parse already registered the references;
/// this path is read-only and additionally catches a present Secret with a
/// missing/non-UTF-8 key and a cross-namespace reference denied by policy.
fn resolve_guardrail_request_secret_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let owner = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        ep.metadata.namespace.clone(),
        ep.metadata.name.clone().unwrap_or_default(),
    );
    let mut names = ep
        .spec
        .request_plugins
        .iter()
        .flatten()
        .filter_map(|entry| match &entry.plugin {
            EdgionPlugin::Guardrail(config) => Some(config.request.collect_secret_ref_names()),
            EdgionPlugin::AiGuard(config) => config
                .evaluator
                .as_ref()
                .map(|evaluator| evaluator.request.collect_secret_ref_names()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    crate::conf_mgr::sync_runtime::resource_processor::credential::request_secret_errors_readonly(
        ctx,
        &owner,
        ResourceKind::EdgionPlugins,
        &names,
    )
}

/// Preserve first-occurrence ordering while removing exact duplicate status
/// errors. Guardrail's read-only classifier intentionally reports
/// SecretNotFound itself, while the generic dependency scan reports the same
/// missing object; the condition must contain it only once.
fn dedup_resolved_ref_errors(errors: &mut Vec<ResolvedRefsError>) {
    let mut seen = std::collections::BTreeSet::new();
    errors.retain(|error| seen.insert((error.reason(), error.message())));
}

impl EdgionPluginsHandler {
    /// Inspect every Secret or ConfigMap this EdgionPlugins depends on (tracked
    /// in `SecretRefManager` by the `parse()` resolvers) and flag missing or
    /// unauthorized material using its encoded target kind.
    fn resolve_secret_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
        let owner_namespace = ep.metadata.namespace.as_deref().unwrap_or("default");
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );

        let mut errors = Vec::new();
        // Sort dependency keys so the joined condition.message is deterministic
        // across reconciles. get_dependencies' backing HashSet has non-deterministic
        // iteration order; without sorting, status_diff::condition_fields_equal
        // (which compares `message`) would trigger spurious K8s status writes.
        let mut deps = ctx.secret_ref_manager().get_dependencies(&resource_ref.key());
        deps.sort();
        for dep_key in deps {
            let (to_kind, object_key) = match dep_key.strip_prefix(CONFIG_MAP_REF_KEY_PREFIX) {
                Some(key) => ("ConfigMap", key),
                None => ("Secret", dep_key.as_str()),
            };
            // object_key is "<ns>/<name>" (or just "<name>" for a
            // namespace-less Secret, per format_secret_key).
            let (ns, name) = match object_key.split_once('/') {
                Some((ns, name)) => (ns.to_string(), name.to_string()),
                None => (String::new(), object_key.to_string()),
            };
            let ns_opt = if ns.is_empty() { None } else { Some(ns.as_str()) };
            if let Some(target_namespace) = ns_opt.filter(|target_namespace| *target_namespace != owner_namespace) {
                let allowed = ctx.runtime().ref_grant_store.check_reference_allowed(
                    owner_namespace,
                    "edgion.io",
                    "EdgionPlugins",
                    target_namespace,
                    "",
                    to_kind,
                    Some(&name),
                );
                if !allowed {
                    errors.push(ResolvedRefsError::RefNotPermitted {
                        from_namespace: owner_namespace.to_string(),
                        from_kind: "EdgionPlugins",
                        target_namespace: target_namespace.to_string(),
                        target_name: name,
                    });
                    continue;
                }
            }
            let missing = match to_kind {
                "ConfigMap" => ctx.runtime().configmap_store.get(ns_opt, &name).is_none(),
                _ => ctx.runtime().secret_store.get(ns_opt, &name).is_none(),
            };
            if missing {
                let namespace = if ns.is_empty() { "default".to_string() } else { ns };
                errors.push(match to_kind {
                    "ConfigMap" => ResolvedRefsError::ConfigMapNotFound { namespace, name },
                    _ => ResolvedRefsError::SecretNotFound { namespace, name },
                });
            }
        }
        errors
    }
}

/// Check every `Wasm` plugin's `source.fetch` and `source.oci` refs
/// (`authHeaderSecretRef`, `pullSecretRef`, and the `fetch.tls` CA /
/// client-cert refs) against the Secret store, re-running the *exact*
/// usable-material checks the Wasm resolver used at `parse` time.
///
/// The generic `resolve_secret_refs` above only asks "does the Secret object
/// exist"; a present-but-unusable Secret (missing/empty/non-UTF-8 key, wrong
/// type) or a cross-namespace ref with no `ReferenceGrant` (the resolver
/// registers the dependency *before* the grant check, so the Secret still
/// looks present) would leave the gateway-visible `resolved_*` field unset
/// while status wrongly reported `ResolvedRefs=True`. The gateway then
/// fail-closes on load. This resolver closes that gap by mapping each failure
/// to the matching `ResolvedRefsError` variant.
///
/// Ordering mirrors the resolver's short-circuit (grant → existence → type →
/// key). A missing Secret object is intentionally *not* reported here — the
/// generic `resolve_secret_refs` already surfaces it as `SecretNotFound`, so
/// deferring avoids a double message on the common not-found path. The
/// `fetch.tls` refs are classified for grant denial only: the CA / client-cert
/// resolver validates no PEM content itself (present + granted is its whole
/// usability bar), so status stays faithful to the resolver rather than
/// stricter than it.
///
/// Accepted limitation (WON'T FIX): no deep dockerconfigjson JSON/credential
/// validation matching the gateway's `select_registry_auth` (malformed JSON,
/// bad base64 `auth`, non-`user:password`, or a registry host absent from
/// `.auths`). A `kubectl create secret docker-registry` document is always
/// well-formed, so host-mismatch is the only realistic failure, and the
/// controller cannot depend on `edgion-gateway` to reuse that parser — not
/// worth the cross-crate refactor. The gateway fail-closes on such a pull;
/// status accuracy is the only cost.
fn resolve_wasm_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let owner_ns = ep.metadata.namespace.as_deref().unwrap_or("default").to_string();
    let mut errors = Vec::new();

    macro_rules! scan_stage {
        ($stage:expr) => {
            if let Some(entries) = &$stage {
                for entry in entries {
                    if let EdgionPlugin::Wasm(config) = &entry.plugin {
                        collect_wasm_ref_errors(config, &owner_ns, ctx, &mut errors);
                    }
                }
            }
        };
    }

    scan_stage!(ep.spec.request_plugins);
    scan_stage!(ep.spec.upstream_response_filter_plugins);
    scan_stage!(ep.spec.upstream_response_body_filter_plugins);
    scan_stage!(ep.spec.upstream_response_plugins);
    errors
}

/// Classify the OCI-pull, fetch-auth, and `fetch.tls` (CA / client-cert) refs
/// of one Wasm plugin, pushing at most one error per ref.
fn collect_wasm_ref_errors(
    config: &edgion_resources::resources::edgion_plugins::plugin_configs::WasmConfig,
    owner_ns: &str,
    ctx: &HandlerContext,
    errors: &mut Vec<ResolvedRefsError>,
) {
    if let Some(oci) = &config.source.oci {
        if let Some(sor) = &oci.pull_secret_ref {
            if let Some(e) = classify_pull_secret_ref(ctx, owner_ns, sor) {
                errors.push(e);
            }
        }
    }
    if let Some(fetch) = &config.source.fetch {
        if let Some(auth) = &fetch.auth_header_secret_ref {
            if let Some(e) = classify_auth_header_ref(ctx, owner_ns, auth) {
                errors.push(e);
            }
        }
        if let Some(tls) = &fetch.tls {
            if let Some(validation) = &tls.outbound.validation {
                if let Some(refs) = &validation.ca_certificate_refs {
                    classify_ca_certificate_refs(ctx, owner_ns, refs, errors);
                }
            }
            if let Some(cc) = &tls.outbound.client_certificate_ref {
                if let Some(e) = wasm_ref_grant_denied(ctx, owner_ns, cc) {
                    errors.push(e);
                }
            }
        }
    }
}

/// Return `RefNotPermitted` when `sor` is a cross-namespace reference with no
/// `ReferenceGrant` permitting it (mirrors `fetch_secret`'s Step-3 check).
fn wasm_ref_grant_denied(
    ctx: &HandlerContext,
    owner_ns: &str,
    sor: &edgion_resources::resources::common::SecretObjectReference,
) -> Option<ResolvedRefsError> {
    if !sor.is_cross_namespace(owner_ns) {
        return None;
    }
    wasm_grant_denied(ctx, owner_ns, sor.effective_namespace(owner_ns), "Secret", &sor.name)
}

/// Grant-check core shared by the Wasm ref classifiers: returns
/// `RefNotPermitted` when a cross-namespace ref into `to_ns` (kind `to_kind`,
/// core API group) has no matching `ReferenceGrant`. Callers must pass a
/// `to_ns` already known to differ from `owner_ns`.
fn wasm_grant_denied(
    ctx: &HandlerContext,
    owner_ns: &str,
    to_ns: &str,
    to_kind: &str,
    to_name: &str,
) -> Option<ResolvedRefsError> {
    let allowed = ctx.runtime().ref_grant_store.check_reference_allowed(
        owner_ns,
        "edgion.io",
        "EdgionPlugins",
        to_ns,
        "",
        to_kind,
        Some(to_name),
    );
    if allowed {
        None
    } else {
        Some(ResolvedRefsError::RefNotPermitted {
            from_namespace: owner_ns.to_string(),
            from_kind: "EdgionPlugins",
            target_namespace: to_ns.to_string(),
            target_name: to_name.to_string(),
        })
    }
}

/// Classify the CA-certificate refs of a Wasm `fetch.tls`. Each ref targets a
/// core Secret or ConfigMap; a cross-namespace ref with no `ReferenceGrant`
/// yields `RefNotPermitted`, mirroring `resolve_ca_certificate_refs`'s
/// GrantGated path (which drops such a ref from `resolved_ca_certificates`).
/// Non-core or unknown-kind refs are skipped, matching the resolver.
fn classify_ca_certificate_refs(
    ctx: &HandlerContext,
    owner_ns: &str,
    refs: &[edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef],
    errors: &mut Vec<ResolvedRefsError>,
) {
    for ca_ref in refs {
        if !edgion_resources::resources::common::is_core_api_group(&ca_ref.group)
            || (ca_ref.kind != "Secret" && ca_ref.kind != "ConfigMap")
        {
            continue;
        }
        let to_ns = ca_ref.namespace.as_deref().unwrap_or(owner_ns);
        if to_ns == owner_ns {
            continue;
        }
        if let Some(e) = wasm_grant_denied(ctx, owner_ns, to_ns, &ca_ref.kind, &ca_ref.name) {
            errors.push(e);
        }
    }
}

/// Classify a Wasm `authHeaderSecretRef`. `None` = usable (or the Secret object
/// is absent, deferred to the generic not-found path).
fn classify_auth_header_ref(
    ctx: &HandlerContext,
    owner_ns: &str,
    auth: &edgion_resources::resources::edgion_plugins::plugin_configs::WasmAuthHeaderSecretRef,
) -> Option<ResolvedRefsError> {
    if let Some(e) = wasm_ref_grant_denied(ctx, owner_ns, &auth.secret) {
        return Some(e);
    }
    let ns = auth.secret.effective_namespace(owner_ns);
    // Absent Secret object → SecretNotFound is emitted by `resolve_secret_refs`.
    let secret = ctx.runtime().secret_store.get(Some(ns), &auth.secret.name)?;
    // The configured `key` is operator-defined (not `&'static`), so its usable-
    // material failure can't ride `MissingKeys(Vec<&'static str>)`; carry the
    // key name in `DecodeError.detail` instead. `read_secret_utf8` collapses
    // missing / empty / non-UTF-8 into one outcome — exactly what the resolver
    // did — so the message enumerates all three.
    if cred::read_secret_utf8(&secret, &[auth.key.as_str()]).is_none() {
        return Some(ResolvedRefsError::DecodeError {
            key: "authHeaderSecretRef",
            detail: format!("Secret data key '{}' is missing, empty, or not UTF-8", auth.key),
        });
    }
    None
}

/// Classify a Wasm `oci.pullSecretRef`. `None` = usable (or the Secret object
/// is absent, deferred to the generic not-found path).
fn classify_pull_secret_ref(
    ctx: &HandlerContext,
    owner_ns: &str,
    sor: &edgion_resources::resources::common::SecretObjectReference,
) -> Option<ResolvedRefsError> {
    if let Some(e) = wasm_ref_grant_denied(ctx, owner_ns, sor) {
        return Some(e);
    }
    let ns = sor.effective_namespace(owner_ns);
    let secret = ctx.runtime().secret_store.get(Some(ns), &sor.name)?;
    if secret.type_.as_deref() != Some("kubernetes.io/dockerconfigjson") {
        return Some(ResolvedRefsError::InvalidSecretType {
            got: secret.type_.clone().unwrap_or_default(),
            allowed: &["kubernetes.io/dockerconfigjson"],
        });
    }
    if cred::read_secret_utf8(&secret, &[".dockerconfigjson"]).is_none() {
        return Some(ResolvedRefsError::DecodeError {
            key: ".dockerconfigjson",
            detail: "Secret key .dockerconfigjson is missing, empty, or not UTF-8".to_string(),
        });
    }
    None
}

/// Resolve every named WAF profile. Required sources are atomic; optional sources
/// join only when the resulting complete generation passes strict compilation.
fn resolve_waf_bundles(ep: &mut EdgionPlugins, ctx: &HandlerContext) {
    let namespace = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(waf) = ep.spec.waf.as_mut() else {
        return;
    };
    let policy_namespace = waf.policy_ref.namespace.as_deref().unwrap_or(namespace).to_string();
    let policy_key = format!("{policy_namespace}/{}", waf.policy_ref.name);
    let selector_key = waf.active_profile_ref.as_ref().map(|reference| {
        format!(
            "{}/{}",
            reference.namespace.as_deref().unwrap_or(namespace),
            reference.name
        )
    });
    waf.policy_ref.ref_denied = None;
    if let Some(reference) = waf.active_profile_ref.as_mut() {
        reference.ref_denied = None;
    }
    waf.policy_resolved = Some(false);
    waf.resolved_policy = None;
    waf.owner_namespace = None;
    waf.selected_profile = None;
    waf.resolved_bundles = None;
    waf.resolved_ref_indices.clear();
    waf.resolution_errors.clear();
    waf.resolution_warnings.clear();
    waf.selector_resolved = None;
    if waf.validate_shape().is_err() {
        return;
    }
    let (policy_target, captured) = ctx
        .runtime()
        .edgion_config_data_store
        .get_waf_policy_snapshot(&policy_key, selector_key.as_deref());
    let Some(policy_target) = policy_target else {
        return;
    };
    let ConfigEntry::WafPolicy(mut policy) = policy_target.spec.data else {
        return;
    };
    let allowed = crate::conf_mgr::sync_runtime::resource_processor::ref_grant::is_edgion_config_data_ref_allowed(
        ctx.runtime(),
        namespace,
        "EdgionPlugins",
        &policy_namespace,
        &waf.policy_ref.name,
        policy_target.spec.visibility,
    );
    if !allowed {
        waf.policy_ref.ref_denied = Some(RefDenied {
            target_namespace: policy_namespace,
            target_name: waf.policy_ref.name.clone(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        });
        return;
    }
    if !policy_target.spec.enable
        || policy_target.spec.active.is_some()
        || policy_target
            .spec
            .unknown_fields
            .iter()
            .any(|path| path.starts_with("data.config"))
        || policy.validate_shape().is_err()
    {
        return;
    }
    waf.policy_resolved = Some(true);
    waf.owner_namespace = Some(policy_namespace.clone());
    let selector_target = waf.active_profile_ref.as_mut().and_then(|reference| {
        reference.ref_denied = None;
        let target_namespace = reference.namespace.as_deref().unwrap_or(namespace);
        let target = captured
            .get(&format!("{target_namespace}/{}", reference.name))
            .and_then(Option::as_ref)?;
        let allowed = crate::conf_mgr::sync_runtime::resource_processor::ref_grant::is_edgion_config_data_ref_allowed(
            ctx.runtime(),
            namespace,
            "EdgionPlugins",
            target_namespace,
            &reference.name,
            target.spec.visibility,
        );
        if !allowed {
            reference.ref_denied = Some(RefDenied {
                target_namespace: target_namespace.to_string(),
                target_name: reference.name.clone(),
                reason: Some("NoMatchingReferenceGrant".to_string()),
            });
            return None;
        }
        target.spec.enable.then_some(target)
    });
    let selector_usable = selector_target
        .as_ref()
        .is_some_and(|target| matches!(&target.spec.data, ConfigEntry::Selector(_)));
    if waf.active_profile_ref.is_some() {
        waf.selector_resolved = Some(selector_usable);
    }
    let selector_active = selector_target.and_then(|target| match &target.spec.data {
        ConfigEntry::Selector(selector) => selector.active.clone(),
        _ => None,
    });
    if let Some(name) = selector_active.as_deref() {
        if !policy.profiles.contains_key(name) {
            waf.resolution_warnings
                .push("WAF Selector names an unknown profile; using defaultProfile".to_string());
        }
    } else if waf.active_profile_ref.is_some() && !selector_usable {
        waf.resolution_warnings
            .push("WAF Selector unavailable or inactive; using defaultProfile".to_string());
    }
    let requested = selector_active.filter(|name| policy.profiles.contains_key(name));
    let desired_profile = requested.unwrap_or_else(|| policy.default_profile.clone());

    let mut usable = std::collections::BTreeMap::new();
    let mut failures = std::collections::BTreeMap::new();
    let names: Vec<_> = policy.profiles.keys().cloned().collect();
    for profile_name in names {
        let mut required_indices = Vec::new();
        let mut required_bundles = Vec::new();
        let mut optional = Vec::new();
        let mut errors = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let profile = policy
            .profiles
            .get_mut(&profile_name)
            .expect("profile key was collected");
        for (index, reference) in profile.bundle_refs.iter_mut().enumerate() {
            let target_namespace = reference
                .target
                .namespace
                .as_deref()
                .unwrap_or(&policy_namespace)
                .to_string();
            let key = format!("{}/{}", target_namespace, reference.target.name);
            reference.target.ref_denied = None;
            let mut resolved = None;
            let kind =
                if !seen.insert(key.clone()) {
                    Some(WafBundleResolutionKind::Invalid)
                } else if let Some(target) = captured.get(&key).and_then(Option::as_ref) {
                    if let ConfigEntry::WafRuleBundle(data) = &target.spec.data {
                        let allowed =
                        crate::conf_mgr::sync_runtime::resource_processor::ref_grant::is_edgion_config_data_ref_allowed(
                            ctx.runtime(), &policy_namespace, "EdgionConfigData", &target_namespace,
                            &reference.target.name, target.spec.visibility,
                        );
                        if !allowed {
                            reference.target.ref_denied = Some(RefDenied {
                                target_namespace: target_namespace.clone(),
                                target_name: reference.target.name.clone(),
                                reason: Some("NoMatchingReferenceGrant".to_string()),
                            });
                            Some(WafBundleResolutionKind::Denied)
                        } else if !target.spec.enable || target.spec.active.is_some() {
                            Some(WafBundleResolutionKind::Disabled)
                        } else if target
                            .spec
                            .unknown_fields
                            .iter()
                            .any(|path| path.starts_with("data.config"))
                            || data.validate().is_err()
                        {
                            Some(WafBundleResolutionKind::Invalid)
                        } else if data.digest != reference.expected_digest {
                            Some(WafBundleResolutionKind::DigestMismatch)
                        } else {
                            resolved = Some(ResolvedWafBundle {
                                identity: key,
                                digest: data.digest.clone(),
                                data: data.clone(),
                            });
                            None
                        }
                    } else {
                        Some(WafBundleResolutionKind::WrongType)
                    }
                } else {
                    Some(WafBundleResolutionKind::Missing)
                };
            if let Some(kind) = kind {
                if reference.optional {
                    if waf.resolution_warnings.len() < 16 {
                        waf.resolution_warnings.push(format!(
                            "waf profile {profile_name} optional bundleRefs[{index}] skipped: {kind:?}"
                        ));
                    }
                } else {
                    errors.push(WafBundleResolutionError {
                        profile: profile_name.clone(),
                        index,
                        namespace: target_namespace,
                        name: reference.target.name.clone(),
                        kind,
                    });
                }
            } else if let Some(bundle) = resolved {
                if reference.optional {
                    optional.push((index, bundle));
                } else {
                    required_indices.push(index);
                    required_bundles.push(bundle);
                }
            }
        }
        if errors.is_empty() {
            let mut candidate = waf.clone();
            candidate.resolved_policy = Some(policy.clone());
            candidate.selected_profile = Some(profile_name.clone());
            candidate.resolved_ref_indices = required_indices.clone();
            candidate.resolved_bundles = Some(required_bundles.clone());
            if candidate.compile_generation().is_err() {
                let reference = &policy.profiles[&profile_name].bundle_refs[0];
                errors.push(WafBundleResolutionError {
                    profile: profile_name.clone(),
                    index: 0,
                    namespace: reference
                        .target
                        .namespace
                        .as_deref()
                        .unwrap_or(&policy_namespace)
                        .to_string(),
                    name: reference.target.name.clone(),
                    kind: WafBundleResolutionKind::Invalid,
                });
            } else {
                for (index, bundle) in optional {
                    let mut trial_indices = required_indices.clone();
                    let mut trial_bundles = required_bundles.clone();
                    let position = trial_indices.partition_point(|previous| *previous < index);
                    trial_indices.insert(position, index);
                    trial_bundles.insert(position, bundle);
                    candidate.resolved_ref_indices = trial_indices.clone();
                    candidate.resolved_bundles = Some(trial_bundles.clone());
                    if candidate.compile_generation().is_ok() {
                        required_indices = trial_indices;
                        required_bundles = trial_bundles;
                    } else if waf.resolution_warnings.len() < 16 {
                        waf.resolution_warnings.push(format!(
                            "waf profile {profile_name} optional bundleRefs[{index}] skipped: compile failed"
                        ));
                    }
                }
                // Keep only the accepted positions for inactive profiles. Retaining
                // their owned source copies multiplies memory by profile count.
                usable.insert(profile_name.clone(), required_indices);
            }
        }
        if !errors.is_empty() {
            failures.insert(profile_name, errors);
        }
    }
    if let Some(errors) = failures.remove(&policy.default_profile) {
        waf.resolution_errors = errors;
        waf.resolved_policy = Some(policy);
        return;
    }
    for (name, errors) in failures {
        if waf.resolution_warnings.len() < 16 {
            waf.resolution_warnings
                .push(format!("waf profile {name} unavailable: {:?}", errors[0].kind));
        }
    }
    let selected = if usable.contains_key(&desired_profile) {
        desired_profile
    } else {
        if waf.resolution_warnings.len() < 16 {
            waf.resolution_warnings
                .push("selected WAF profile unavailable; using defaultProfile".to_string());
        }
        policy.default_profile.clone()
    };
    if let Some(indices) = usable.remove(&selected) {
        let profile = &policy.profiles[&selected];
        let bundles: Option<Vec<_>> = indices
            .iter()
            .map(|&index| {
                let reference = profile.bundle_refs.get(index)?;
                let target_namespace = reference.target.namespace.as_deref().unwrap_or(&policy_namespace);
                let identity = format!("{target_namespace}/{}", reference.target.name);
                let target = captured.get(&identity)?.as_ref()?;
                let ConfigEntry::WafRuleBundle(data) = &target.spec.data else {
                    return None;
                };
                Some(ResolvedWafBundle {
                    identity,
                    digest: data.digest.clone(),
                    data: data.clone(),
                })
            })
            .collect();
        if let Some(bundles) = bundles {
            waf.selected_profile = Some(selected);
            waf.resolved_ref_indices = indices;
            waf.resolved_bundles = Some(bundles);
        }
    }
    waf.resolved_policy = Some(policy);
}

fn resolve_waf_active_profile_ref(ep: &EdgionPlugins) -> Vec<ResolvedRefsError> {
    let Some(waf) = ep.spec.waf.as_ref() else {
        return Vec::new();
    };
    if waf.selector_resolved.is_none() {
        return Vec::new();
    }
    let Some(reference) = waf.active_profile_ref.as_ref() else {
        return Vec::new();
    };
    let namespace = ep.metadata.namespace.as_deref().unwrap_or("default");
    let target_namespace = reference.namespace.as_deref().unwrap_or(namespace);
    if reference.ref_denied.is_some() {
        return vec![ResolvedRefsError::RefNotPermitted {
            from_namespace: namespace.to_string(),
            from_kind: "EdgionPlugins",
            target_namespace: target_namespace.to_string(),
            target_name: reference.name.clone(),
        }];
    }
    if waf.selector_resolved == Some(true) {
        Vec::new()
    } else {
        vec![ResolvedRefsError::EdgionConfigDataNotFound {
            namespace: target_namespace.to_string(),
            name: reference.name.clone(),
            expected_type: "Selector".to_string(),
        }]
    }
}

fn resolve_waf_policy_ref(ep: &EdgionPlugins) -> Vec<ResolvedRefsError> {
    let Some(waf) = ep.spec.waf.as_ref() else {
        return Vec::new();
    };
    let namespace = ep.metadata.namespace.as_deref().unwrap_or("default");
    let target_namespace = waf.policy_ref.namespace.as_deref().unwrap_or(namespace);
    if waf.policy_ref.ref_denied.is_some() {
        return vec![ResolvedRefsError::RefNotPermitted {
            from_namespace: namespace.to_string(),
            from_kind: "EdgionPlugins",
            target_namespace: target_namespace.to_string(),
            target_name: waf.policy_ref.name.clone(),
        }];
    }
    if waf.policy_resolved == Some(true) {
        Vec::new()
    } else {
        vec![ResolvedRefsError::EdgionConfigDataNotFound {
            namespace: target_namespace.to_string(),
            name: waf.policy_ref.name.clone(),
            expected_type: "WafPolicy".to_string(),
        }]
    }
}

/// Check each RegionRoute plugin's `overrideRef` against the EdgionConfigData store.
/// Returns one `RefNotPermitted` error when `ref_denied` is set (cross-namespace denial),
/// or one missing/type/content error per unusable reference. Content validation
/// includes this consumer's modulo and does not invalidate its accepted base.
/// Mutually exclusive per ref via else-branch to avoid a TOCTOU double-message.
pub fn resolve_override_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
        if !entry.is_enabled() {
            continue;
        }
        if let EdgionPlugin::RegionRoute(ref config) = entry.plugin {
            if let Some(ref override_ref) = config.override_ref {
                let override_ns = override_ref.namespace.as_deref().unwrap_or(ns);
                if override_ref.ref_denied.is_some() {
                    errors.push(ResolvedRefsError::RefNotPermitted {
                        from_namespace: ns.to_string(),
                        from_kind: "EdgionPlugins",
                        target_namespace: override_ns.to_string(),
                        target_name: override_ref.name.clone(),
                    });
                } else {
                    let target = ctx
                        .runtime()
                        .edgion_config_data_store
                        .get(Some(override_ns), &override_ref.name);
                    let overlay = target.as_ref().and_then(|target| match &target.spec.data {
                        ConfigEntry::RegionRouteOverride(overlay) => Some((target.spec.enable, overlay)),
                        _ => None,
                    });
                    if let Some((enabled, overlay)) = overlay {
                        let error = if !enabled {
                            Some("RegionRouteOverride is disabled; using base regions".to_string())
                        } else {
                            overlay
                                .regions
                                .as_deref()
                                .and_then(|regions| config.validate_effective_regions(regions).err())
                        };
                        if let Some(detail) = error {
                            errors.push(ResolvedRefsError::EdgionConfigDataInvalid {
                                namespace: override_ns.to_string(),
                                name: override_ref.name.clone(),
                                detail,
                            });
                        }
                    } else {
                        errors.push(ResolvedRefsError::EdgionConfigDataNotFound {
                            namespace: override_ns.to_string(),
                            name: override_ref.name.clone(),
                            expected_type: "RegionRouteOverride".to_string(),
                        });
                    }
                }
            }
        }
    }
    errors
}

/// Check each `IpRestriction` plugin's `allow_refs`/`deny_refs` against the EdgionConfigData
/// store. Returns one `RefNotPermitted` error when `ref_denied` is set (cross-namespace denial),
/// or one `EdgionConfigDataNotFound { expected_type: "IpList" }` per missing or wrong-type
/// reference. Mutually exclusive per ref via else-branch to avoid a TOCTOU double-message.
/// `request_plugins` is a Vec (stable order), so no extra sort needed.
pub fn resolve_ip_list_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let mut errors = Vec::new();
    let mut check_ref = |r: &edgion_resources::resources::edgion_config_data::EdgionConfigDataRef| {
        let rns = r.namespace.as_deref().unwrap_or(ns);
        if r.ref_denied.is_some() {
            errors.push(ResolvedRefsError::RefNotPermitted {
                from_namespace: ns.to_string(),
                from_kind: "EdgionPlugins",
                target_namespace: rns.to_string(),
                target_name: r.name.clone(),
            });
        } else {
            let target = ctx.runtime().edgion_config_data_store.get(Some(rns), &r.name);
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
    };
    if let Some(plugins) = &ep.spec.request_plugins {
        for entry in plugins {
            if let EdgionPlugin::IpRestriction(config) = &entry.plugin {
                for reference in config
                    .allow_refs
                    .iter()
                    .flatten()
                    .chain(config.deny_refs.iter().flatten())
                {
                    check_ref(reference);
                }
            }
        }
    }
    // Operator-authored entry-level / RR / body / dye ipRefs fail closed.
    // RAP sidecar ipRefs stay on PartiallyInvalid (existing partial-good).
    super::plugins::condition_tree::for_each_fail_closed_ip_ref(ep, |reference| {
        check_ref(reference);
    });
    errors
}

/// Check each `RequestAccessPolicy` plugin's `active_profile_ref` against the EdgionConfigData
/// store. Returns one `RefNotPermitted` error when `ref_denied` is set (cross-namespace denial),
/// or one `EdgionConfigDataNotFound { expected_type: "Selector" }` per missing or wrong-type
/// reference. Mutually exclusive per ref via else-branch to avoid a TOCTOU double-message.
///
/// This is a net-new resolver — `RequestAccessPolicy.active_profile_ref` (the 7th ref site)
/// had no status reporting before Task 5.
pub fn resolve_active_profile_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
        if let EdgionPlugin::RequestAccessPolicy(ref config) = entry.plugin {
            if let Some(ref r) = config.active_profile_ref {
                let rns = r.namespace.as_deref().unwrap_or(ns);
                if r.ref_denied.is_some() {
                    errors.push(ResolvedRefsError::RefNotPermitted {
                        from_namespace: ns.to_string(),
                        from_kind: "EdgionPlugins",
                        target_namespace: rns.to_string(),
                        target_name: r.name.clone(),
                    });
                } else {
                    let target = ctx.runtime().edgion_config_data_store.get(Some(rns), &r.name);
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

/// Check each `Canary` plugin's `active_profile_ref` against the EdgionConfigData
/// store. Returns one `RefNotPermitted` error when `ref_denied` is set (cross-namespace
/// denial), or one `EdgionConfigDataNotFound { expected_type: "Selector" }` per missing
/// or wrong-type reference. Mirrors `resolve_active_profile_refs` (RequestAccessPolicy).
pub fn resolve_canary_active_profile_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
        if let EdgionPlugin::Canary(ref config) = entry.plugin {
            if let Some(ref r) = config.active_profile_ref {
                let rns = r.namespace.as_deref().unwrap_or(ns);
                if r.ref_denied.is_some() {
                    errors.push(ResolvedRefsError::RefNotPermitted {
                        from_namespace: ns.to_string(),
                        from_kind: "EdgionPlugins",
                        target_namespace: rns.to_string(),
                        target_name: r.name.clone(),
                    });
                } else {
                    let target = ctx.runtime().edgion_config_data_store.get(Some(rns), &r.name);
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

/// Requeue all Gateways, HTTPRoutes, and GRPCRoutes that reference this EdgionPlugins.
async fn requeue_referencing_resources(ep: &EdgionPlugins, ctx: &HandlerContext) {
    let Some(ep_name) = ep.metadata.name.as_deref().filter(|n| !n.is_empty()) else {
        return;
    };
    let ep_key = format!("{}/{}", ep.metadata.namespace.as_deref().unwrap_or("default"), ep_name);
    let refs = ctx.runtime().plugin_ref_mgr.get_refs(&ep_key);
    if !refs.is_empty() {
        tracing::info!(
            ep_key = %ep_key,
            ref_count = refs.len(),
            "EdgionPlugins changed, requeuing referencing resources"
        );
    }
    for resource_ref in refs {
        ctx.requeue(resource_ref.kind_str(), resource_ref.resource_key()).await;
    }
}

/// Collect partial-good entry errors over every inline IpRestriction allow/deny in the
/// request stage (the only stage IpRestriction refs are scanned; see get_ip_list_ref_keys).
fn collect_inline_ip_entry_errors(ep: &EdgionPlugins) -> Vec<edgion_resources::matcher::ip_cidr_helpers::EntryError> {
    use edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors;
    let mut errs = Vec::new();
    let Some(plugins) = &ep.spec.request_plugins else {
        return errs;
    };
    for entry in plugins {
        if let EdgionPlugin::IpRestriction(ref config) = entry.plugin {
            if let Some(g) = &config.allow {
                errs.extend(collect_entry_errors(g));
            }
            if let Some(g) = &config.deny {
                errs.extend(collect_entry_errors(g));
            }
        }
    }
    errs
}

fn request_access_policy_resolution_warnings(ep: &EdgionPlugins) -> Vec<String> {
    ep.spec
        .request_plugins
        .iter()
        .flatten()
        .filter_map(|entry| match &entry.plugin {
            EdgionPlugin::RequestAccessPolicy(config) => Some(config.resolution_warnings.iter().cloned()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins_test.rs"]
mod tests;
