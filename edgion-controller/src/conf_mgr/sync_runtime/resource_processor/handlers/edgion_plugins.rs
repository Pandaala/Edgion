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
    condition_types, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::observe::unknown_fields_registry::bounded_unknown_field_paths;
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsStatus};
use edgion_resources::ResourceKind;

use super::config_data_ref_mark::{mark_one_ref, ExpectedEntry};
use super::plugins::resolve_credential as cred;
use super::san_guard;

pub struct EdgionPluginsHandler;

impl EdgionPluginsHandler {
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
                | EdgionPlugin::RateLimit(_)
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
                | EdgionPlugin::TraceContext(_)
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
        // Plugin policy: clear any inbound policy_action and re-stamp from the
        // compiled policy. Runs before validation; hits surface via the
        // PluginPolicyRestricted condition in update_status, NOT as
        // validation errors (Accepted keeps meaning "valid and synced").
        let _ = super::super::plugin_policy_stamp::stamp_http_entries(ep);

        // Unknown-field diagnostic: `spec.unknown_fields` was already
        // populated by the ingest hook, loader, or Admin API seam, so this
        // is a pure read — no re-walk here. Warn once on the apply path; the
        // status-side UnknownConfigFields condition is stamped in
        // update_status (which has no preparse-local state, per the same
        // pattern as PluginPolicyRestricted). Advisory only: this never
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

        let mut errors = ep.validate_plugins();
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

        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().link_sys_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime()
            .credential_source_registry
            .remove_consumer(&resource_ref.resource_key());
        // One cross-namespace index covers both Secret and EdgionConfigData
        // references. Clear it before any resolver starts rebuilding either
        // class; clearing later would erase Secret refs registered above the
        // EdgionConfigData pass and prevent ReferenceGrant revalidation.
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);

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
        {
            if let Some((target_ns, _)) = key.split_once('/') {
                if target_ns != ns {
                    cross_mgr.add_ref(target_ns.to_string(), resource_ref.clone());
                }
            }
        }

        // Mark cross-namespace EdgionConfigData refs as denied when no ReferenceGrant permits them.
        Self::mark_config_data_ref_denials(&mut ep, ctx);

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
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().link_sys_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime()
            .credential_source_registry
            .forget_consumer(&resource_ref.resource_key());
        Self::clear_config_data_refs(ep, ctx);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);
        requeue_referencing_resources(ep, ctx).await;

        tracing::debug!(
            edgion_plugins = %resource_ref.key(),
            "Cleared secret, LinkSys, EdgionConfigData, and cross-ns references on EdgionPlugins delete"
        );
    }

    fn update_status(&self, ep: &mut EdgionPlugins, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ep.metadata.generation;

        // Accepted reflects preparse + validate errors (already merged by processor).
        let accepted = if validation_errors.is_empty() {
            condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Resource accepted",
                generation,
            )
        } else {
            condition_false(
                condition_types::ACCEPTED,
                condition_reasons::INVALID,
                validation_errors.join("; "),
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
        resolved_errors.extend(resolve_secret_match_refs(ep, ctx));
        resolved_errors.extend(resolve_guardrail_request_secret_refs(ep, ctx));
        resolved_errors.extend(resolve_wasm_refs(ep, ctx));
        resolved_errors.extend(resolve_override_refs(ep, ctx));
        resolved_errors.extend(resolve_ip_list_refs(ep, ctx));
        resolved_errors.extend(resolve_active_profile_refs(ep, ctx));
        resolved_errors.extend(resolve_canary_active_profile_refs(ep, ctx));
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
        let partially_invalid = if partial_messages.is_empty() {
            partially_invalid_condition(None, generation)
        } else {
            partially_invalid_condition(Some(partial_messages.join("; ")), generation)
        };

        // Computed before the mutable `status` borrow below (collect_policy_hits_http
        // needs a shared borrow of `ep`, which would otherwise overlap it).
        let policy_hits = super::super::plugin_policy_stamp::collect_policy_hits_http(ep);

        let status = ep
            .status
            .get_or_insert_with(|| EdgionPluginsStatus { conditions: vec![] });
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
        update_condition(&mut status.conditions, partially_invalid);
        super::super::plugin_policy_stamp::apply_policy_condition(&mut status.conditions, &policy_hits, generation);
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
    /// Inspect every Secret this EdgionPlugins depends on (tracked in `SecretRefManager`
    /// by the `parse()` resolvers) and flag any that aren't present in the Secret store.
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
            // dep_key is "<ns>/<name>" (or just "<name>" if namespace-less, per format_secret_key).
            let (ns, name) = match dep_key.split_once('/') {
                Some((ns, name)) => (ns.to_string(), name.to_string()),
                None => (String::new(), dep_key.clone()),
            };
            let ns_opt = if ns.is_empty() { None } else { Some(ns.as_str()) };
            if let Some(target_namespace) = ns_opt.filter(|target_namespace| *target_namespace != owner_namespace) {
                let allowed = ctx.runtime().ref_grant_store.check_reference_allowed(
                    owner_namespace,
                    "edgion.io",
                    "EdgionPlugins",
                    target_namespace,
                    "",
                    "Secret",
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
            if ctx.runtime().secret_store.get(ns_opt, &name).is_none() {
                errors.push(ResolvedRefsError::SecretNotFound {
                    namespace: if ns.is_empty() { "default".to_string() } else { ns },
                    name,
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
        if !entry.is_enabled() || entry.policy_action.is_some() {
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
mod tests {
    use super::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{
        HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
    };
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::prelude_resources::EdgionConfigData;
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigDataRef, EdgionConfigDataSpec, RegionRouteOverride,
    };
    use edgion_resources::resources::edgion_plugins::plugin_configs::KeyAuthSecretGroup;
    use edgion_resources::resources::edgion_plugins::{
        BasicAuthConfig, BasicAuthSecretGroup, CertSourceMode, CredentialInjectionTarget, CredentialInjectorConfig,
        EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, HeaderCertAuthConfig, HmacAuthConfig, HmacAuthSecretGroup,
        JweDecryptConfig, JwtAlgorithm, JwtAuthConfig, JwtIssuerGroup, KeyAuthConfig, KeyMetadata, OpenidConnectConfig,
        RegionRouteConfig, RequestFilterEntry, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
    };
    use edgion_resources::resources::gateway::SecretObjectReference;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    fn make_ctx_with_secrets(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
        (ctx, runtime)
    }

    fn build_secret(namespace: &str, name: &str, key: &str, value: &str) -> Secret {
        let mut data = BTreeMap::new();
        data.insert(key.to_string(), ByteString(value.as_bytes().to_vec()));
        Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn credential_injector_cross_namespace_grant_dependency_survives_parse() {
        let secret = build_secret("shared", "provider-token", "bearerToken", "shared-token");
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::from([("shared/provider-token".to_string(), secret)]));
        let mut ep = EdgionPlugins::new("injector", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("tenant".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::CredentialInjector(
            CredentialInjectorConfig {
                credential_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "provider-token".to_string(),
                    namespace: Some("shared".to_string()),
                },
                target: CredentialInjectionTarget::Bearer,
                resolved_credential: None,
            },
        ))]);

        let mut parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut parsed, &ctx, &[]);

        let owner = ResourceRef::new(
            edgion_resources::ResourceKind::EdgionPlugins,
            Some("tenant".to_string()),
            "injector".to_string(),
        );
        assert!(runtime.cross_ns_ref_mgr.get_refs("shared").contains(&owner));
        let resolved = parsed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, "RefNotPermitted");
    }

    fn injected_key_auth_config(secret_groups: Option<Vec<KeyAuthSecretGroup>>) -> KeyAuthConfig {
        KeyAuthConfig {
            secret_groups,
            resolved_keys: Some(HashMap::from([(
                "injected-key".to_string(),
                KeyMetadata {
                    headers: HashMap::new(),
                    group: "injected".to_string(),
                },
            )])),
            ..Default::default()
        }
    }

    fn injected_jwt_auth_config(issuer_groups: Option<Vec<JwtIssuerGroup>>) -> JwtAuthConfig {
        JwtAuthConfig {
            anonymous: issuer_groups.is_none().then(|| "guest".to_string()),
            issuer_groups,
            resolved_groups_by_iss: Some(HashMap::from([(
                "https://injected.example".to_string(),
                ResolvedJwtIssuerGroup {
                    group: "injected".to_string(),
                    algorithm: JwtAlgorithm::HS256,
                    audiences: None,
                    credentials: HashMap::from([(
                        "injected-kid".to_string(),
                        ResolvedJwtCredential {
                            key: Some("injected-kid".to_string()),
                            secret: Some("injected-secret".to_string()),
                            public_key: None,
                        },
                    )]),
                },
            )])),
            ..Default::default()
        }
    }

    fn assert_auth_resolved_fields_cleared(ep: &EdgionPlugins) {
        let plugins = ep.spec.request_plugins.as_ref().expect("request plugins");
        match &plugins[0].plugin {
            EdgionPlugin::KeyAuth(config) => assert!(config.resolved_keys.is_none()),
            _ => panic!("expected KeyAuth"),
        }
        match &plugins[1].plugin {
            EdgionPlugin::JwtAuth(config) => assert!(config.resolved_groups_by_iss.is_none()),
            _ => panic!("expected JwtAuth"),
        }
    }

    #[tokio::test]
    async fn parse_clears_injected_auth_credentials_without_source_refs() {
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ep = EdgionPlugins::new("injected-auth", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![
            RequestFilterEntry::new(EdgionPlugin::KeyAuth(injected_key_auth_config(None))),
            RequestFilterEntry::new(EdgionPlugin::JwtAuth(injected_jwt_auth_config(None))),
        ]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        assert_auth_resolved_fields_cleared(&parsed);
    }

    #[tokio::test]
    async fn parse_clears_injected_auth_credentials_when_secrets_are_missing() {
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());
        let missing_ref = SecretObjectReference {
            group: None,
            kind: None,
            name: "missing-auth-secret".to_string(),
            namespace: None,
        };
        let mut ep = EdgionPlugins::new("injected-auth", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![
            RequestFilterEntry::new(EdgionPlugin::KeyAuth(injected_key_auth_config(Some(vec![
                KeyAuthSecretGroup {
                    name: "keys".to_string(),
                    description: None,
                    secret_refs: vec![missing_ref.clone()],
                },
            ])))),
            RequestFilterEntry::new(EdgionPlugin::JwtAuth(injected_jwt_auth_config(Some(vec![
                JwtIssuerGroup {
                    name: "tokens".to_string(),
                    description: None,
                    issuers: vec!["https://issuer.example".to_string()],
                    secret_refs: vec![missing_ref],
                    algorithm: None,
                    audiences: None,
                },
            ])))),
        ]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        assert_auth_resolved_fields_cleared(&parsed);
    }

    fn request_access_policy_config(
        active_profile_ref: Option<EdgionConfigDataRef>,
        condition: Option<edgion_resources::resources::edgion_plugins::plugin_configs::Condition>,
    ) -> edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessPolicyConfig {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            ConditionSet, IpAccessMatch, IpAccessRuleConfig, RequestAccessProfile, RequestAccessRule,
            RequestAccessRuleType, RequiredRuleGroup,
        };
        edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessPolicyConfig {
            default_profile: "lockdown".to_string(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                RequestAccessProfile {
                    required_rule_groups: vec![RequiredRuleGroup {
                        name: "source".to_string(),
                        any_of_rules: vec![RequestAccessRule {
                            name: "test".to_string(),
                            rule: RequestAccessRuleType::DirectPeerIp(IpAccessRuleConfig {
                                ip: IpAccessMatch {
                                    ips: vec!["0.0.0.0/0".to_string(), "::/0".to_string()],
                                    ..Default::default()
                                },
                                conditions: condition.map(|condition| ConditionSet::all_of(vec![condition])),
                            }),
                        }],
                    }],
                },
            )]),
            active_profile_ref,
            status: 403,
            message: None,
            description: None,
            resolved_profiles: None,
            resolution_warnings: Vec::new(),
        }
    }

    fn first_request_access_policy_condition(
        config: &edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessPolicyConfig,
    ) -> &edgion_resources::resources::edgion_plugins::plugin_configs::Condition {
        use edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessRuleType;
        let rule = &config.runtime_profiles()["lockdown"].required_rule_groups[0].any_of_rules[0];
        let conditions = match &rule.rule {
            RequestAccessRuleType::DirectPeerIp(ip) | RequestAccessRuleType::RemoteIp(ip) => ip.conditions.as_ref(),
            RequestAccessRuleType::Url(url) => url.conditions.as_ref(),
        };
        conditions
            .and_then(|conditions| conditions.iter_conditions().next())
            .expect("request access rule condition")
    }

    fn request_access_url_policy(config_refs_yaml: &str) -> EdgionPlugins {
        let config: edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessPolicyConfig =
            serde_yaml::from_str(&format!(
                r#"
defaultProfile: lockdown
profiles:
  lockdown:
    requiredRuleGroups:
      - name: callbacks
        anyOfRules:
          - name: referenced-vendors
            type: url
            config:
              configRefs:
{config_refs_yaml}
"#
            ))
            .expect("valid RequestAccessPolicy URL fixture");
        let mut ep = EdgionPlugins::new("url-policy", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(config))]);
        ep
    }

    fn request_access_url_data(
        namespace: &str,
        name: &str,
        candidates: Vec<edgion_resources::resources::edgion_plugins::plugin_configs::UrlAccessCandidate>,
    ) -> EdgionConfigData {
        use edgion_resources::resources::edgion_config_data::RequestAccessUrlAllowListData;
        build_edgion_config_data(
            namespace,
            name,
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::RequestAccessUrlAllowList(RequestAccessUrlAllowListData { items: candidates }),
            },
        )
    }

    fn request_access_url_candidate(
        name: &str,
        conditions: Vec<edgion_resources::resources::edgion_plugins::plugin_configs::Condition>,
    ) -> edgion_resources::resources::edgion_plugins::plugin_configs::UrlAccessCandidate {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AccessPathMatch, AccessPathMatchType, ConditionSet,
        };
        edgion_resources::resources::edgion_plugins::plugin_configs::UrlAccessCandidate {
            name: name.to_string(),
            hosts: vec!["callbacks.example.com".to_string()],
            paths: vec![AccessPathMatch {
                match_type: AccessPathMatchType::PathPrefix,
                value: format!("/callbacks/{name}"),
            }],
            conditions: (!conditions.is_empty()).then(|| ConditionSet::all_of(conditions)),
        }
    }

    fn first_resolved_url_candidates(
        ep: &EdgionPlugins,
    ) -> &[edgion_resources::resources::edgion_plugins::plugin_configs::UrlAccessCandidate] {
        use edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessRuleType;
        let EdgionPlugin::RequestAccessPolicy(config) = &ep.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected RequestAccessPolicy")
        };
        let RequestAccessRuleType::Url(url) =
            &config.runtime_profiles()["lockdown"].required_rule_groups[0].any_of_rules[0].rule
        else {
            panic!("expected URL rule")
        };
        url.resolved_candidates.as_deref().expect("resolved URL candidates")
    }

    fn first_url_candidate_condition(
        candidate: &edgion_resources::resources::edgion_plugins::plugin_configs::UrlAccessCandidate,
    ) -> &edgion_resources::resources::edgion_plugins::plugin_configs::Condition {
        candidate
            .conditions
            .as_ref()
            .and_then(|conditions| conditions.iter_conditions().next())
            .expect("URL candidate condition")
    }

    fn build_basic_auth_secret(namespace: &str, name: &str, username: &str, password: &str) -> Secret {
        let mut data = BTreeMap::new();
        data.insert("username".to_string(), ByteString(username.as_bytes().to_vec()));
        data.insert("password".to_string(), ByteString(password.as_bytes().to_vec()));
        Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_parse_resolves_basic_auth_users_and_registers_refs() {
        let alice = build_basic_auth_secret("default", "basic-auth-alice", "alice", "alice-password");
        let bob = build_basic_auth_secret("default", "basic-auth-bob", "bob", "bob-password");
        let mut secrets = HashMap::new();
        secrets.insert("default/basic-auth-alice".to_string(), alice);
        secrets.insert("default/basic-auth-bob".to_string(), bob);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("basic-auth-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::BasicAuth(
            BasicAuthConfig {
                secret_groups: Some(vec![BasicAuthSecretGroup {
                    name: "default".to_string(),
                    description: None,
                    secret_refs: vec![
                        SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "basic-auth-alice".to_string(),
                            namespace: None,
                        },
                        SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "basic-auth-bob".to_string(),
                            namespace: None,
                        },
                    ],
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::BasicAuth(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        let resolved_users = config.resolved_users.as_ref().expect("expected resolved users");
        assert_eq!(
            resolved_users.get("alice").map(|v| v.password.as_str()),
            Some("alice-password")
        );
        assert_eq!(
            resolved_users.get("bob").map(|v| v.password.as_str()),
            Some("bob-password")
        );
        assert_eq!(ctx.secret_ref_manager().get_refs("default/basic-auth-alice").len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("default/basic-auth-bob").len(), 1);
    }

    #[tokio::test]
    async fn basic_auth_duplicate_warnings_omit_credentials_and_preserve_first_wins() {
        use std::io::{Read, Seek};
        use tracing::instrument::WithSubscriber;

        let username = "private-user@example.invalid";
        let credentials = [
            ("first-user", "first-password"),
            ("duplicate-same", "same-group-password"),
            ("duplicate-other", "other-group-password"),
        ];
        let secrets = credentials
            .iter()
            .map(|(name, password)| {
                (
                    format!("default/{name}"),
                    build_basic_auth_secret("default", name, username, password),
                )
            })
            .collect();
        let (ctx, _runtime) = make_ctx_with_secrets(secrets);
        let groups = [
            ("group-a", vec!["first-user", "duplicate-same"]),
            ("group-b", vec!["duplicate-other"]),
        ]
        .into_iter()
        .map(|(name, refs)| BasicAuthSecretGroup {
            name: name.to_string(),
            description: None,
            secret_refs: refs
                .into_iter()
                .map(|name| SecretObjectReference {
                    group: None,
                    kind: None,
                    name: name.to_string(),
                    namespace: None,
                })
                .collect(),
        })
        .collect();
        let mut ep = EdgionPlugins::new("basic-auth-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::BasicAuth(
            BasicAuthConfig {
                secret_groups: Some(groups),
                ..Default::default()
            },
        ))]);

        let mut log_file = tempfile::tempfile().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log_file.try_clone().unwrap())
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .finish();
        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).with_subscriber(subscriber).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        log_file.rewind().unwrap();
        let mut logs = String::new();
        log_file.read_to_string(&mut logs).unwrap();

        let warnings: Vec<_> = logs.lines().filter(|line| line.contains("BasicAuth:")).collect();
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("Duplicate username within group, skipping"));
        assert!(warnings[0].contains("secret_key=default/duplicate-same"));
        assert!(warnings[0].contains("group=group-a"));
        assert!(warnings[1].contains("duplicate username across groups; first-wins kept"));
        assert!(warnings[1].contains("secret_key=default/duplicate-other"));
        assert!(warnings[1].contains("existing_group=group-a"));
        assert!(warnings[1].contains("incoming_group=group-b"));
        assert!(!logs.contains(username));
        assert!(!logs.contains("username="));
        for (name, password) in credentials {
            assert!(!logs.contains(password));
            assert_eq!(ctx.secret_ref_manager().get_refs(&format!("default/{name}")).len(), 1);
        }

        let EdgionPlugin::BasicAuth(config) = &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected BasicAuth");
        };
        let users = config.resolved_users.as_ref().unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[username].password, "first-password");
        assert_eq!(users[username].group, "group-a");
    }

    #[tokio::test]
    async fn credential_parse_warnings_omit_yaml_scalars_and_jwt_identities() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            JwtAuthConfig, JwtIssuerGroup, KeyAuthConfig, KeyAuthSecretGroup,
        };
        use std::io::{Read, Seek};
        use tracing::instrument::WithSubscriber;

        let scalar = "private-yaml-credential-marker";
        let kid = "private-jwt-identity@example.invalid";
        let mut secrets = HashMap::new();
        for (name, field, value) in [
            ("bad-hmac", "credentials.yaml", scalar),
            ("bad-key", "keys.yaml", scalar),
            ("good-key", "keys.yaml", "- key: preserved-api-key\n"),
            ("jwt-first", "key", kid),
            ("jwt-duplicate", "key", kid),
            ("jwt-missing", "key", kid),
        ] {
            secrets.insert(format!("default/{name}"), build_secret("default", name, field, value));
        }
        for (name, value) in [
            ("jwt-first", "  preserved-jwt-secret  "),
            ("jwt-duplicate", "  rejected-jwt-secret  "),
        ] {
            secrets
                .get_mut(&format!("default/{name}"))
                .unwrap()
                .data
                .as_mut()
                .unwrap()
                .insert("secret".into(), k8s_openapi::ByteString(value.as_bytes().to_vec()));
        }
        let secret_ref = |name: &str| SecretObjectReference {
            group: None,
            kind: None,
            namespace: None,
            name: name.to_owned(),
        };
        let (ctx, _runtime) = make_ctx_with_secrets(secrets);
        let mut ep = EdgionPlugins::new("credential-parse", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![
            RequestFilterEntry::new(EdgionPlugin::HmacAuth(HmacAuthConfig {
                match_config: edgion_resources::resources::edgion_plugins::HmacMatchConfig {
                    secret_groups: Some(vec![HmacAuthSecretGroup {
                        name: "auth-group".into(),
                        description: None,
                        secret_refs: vec![secret_ref("bad-hmac")],
                    }]),
                    ..Default::default()
                },
                ..Default::default()
            })),
            RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig {
                secret_groups: Some(vec![KeyAuthSecretGroup {
                    name: "auth-group".into(),
                    description: None,
                    secret_refs: vec![secret_ref("bad-key"), secret_ref("good-key")],
                }]),
                ..Default::default()
            })),
            RequestFilterEntry::new(EdgionPlugin::JwtAuth(JwtAuthConfig {
                issuer_groups: Some(vec![JwtIssuerGroup {
                    name: "auth-group".into(),
                    description: None,
                    issuers: vec!["https://issuer.example".into()],
                    algorithm: None,
                    audiences: None,
                    secret_refs: vec![
                        secret_ref("jwt-first"),
                        secret_ref("jwt-duplicate"),
                        secret_ref("jwt-missing"),
                    ],
                }]),
                ..Default::default()
            })),
        ]);
        let mut log_file = tempfile::tempfile().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log_file.try_clone().unwrap())
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .finish();
        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).with_subscriber(subscriber).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        log_file.rewind().unwrap();
        let mut logs = String::new();
        log_file.read_to_string(&mut logs).unwrap();
        assert!(!logs.contains(scalar));
        assert!(!logs.contains(kid));
        assert!(!logs.contains("preserved-jwt-secret"));
        assert!(!logs.contains("rejected-jwt-secret"));
        for (name, message) in [
            ("bad-hmac", "Failed to parse credentials.yaml"),
            ("bad-key", "Failed to parse keys.yaml"),
            ("jwt-duplicate", "Duplicate kid within group"),
            ("jwt-missing", "Secret missing both 'secret' and 'publicKey'"),
        ] {
            let line = logs.lines().find(|line| line.contains(message)).unwrap();
            assert!(line.contains(&format!("secret_key=default/{name}")));
            assert!(line.contains("group=auth-group"));
            if name.starts_with("bad-") {
                assert!(line.contains("line=1"));
                assert!(line.contains("column=1"));
            }
        }
        let entries = parsed.spec.request_plugins.as_ref().unwrap();
        let EdgionPlugin::KeyAuth(config) = &entries[1].plugin else {
            panic!("expected KeyAuth")
        };
        assert!(config.resolved_keys.as_ref().unwrap().contains_key("preserved-api-key"));
        let EdgionPlugin::JwtAuth(config) = &entries[2].plugin else {
            panic!("expected JwtAuth")
        };
        let credentials = &config.resolved_groups_by_iss.as_ref().unwrap()["https://issuer.example"].credentials;
        assert_eq!(credentials.len(), 1);
        assert_eq!(credentials[kid].secret.as_deref(), Some("preserved-jwt-secret"));
        for name in [
            "bad-hmac",
            "bad-key",
            "good-key",
            "jwt-first",
            "jwt-duplicate",
            "jwt-missing",
        ] {
            assert_eq!(ctx.secret_ref_manager().get_refs(&format!("default/{name}")).len(), 1);
        }
    }

    #[tokio::test]
    async fn test_parse_resolves_openid_connect_secrets_and_registers_refs() {
        let client_secret = build_secret("default", "oidc-client", "clientSecret", "client-secret-value");
        let session_secret = build_secret("default", "oidc-session", "sessionSecret", "session-secret-value");
        let mut secrets = HashMap::new();
        secrets.insert("default/oidc-client".to_string(), client_secret);
        secrets.insert("default/oidc-session".to_string(), session_secret);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("oidc-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::OpenidConnect(Box::new(
            OpenidConnectConfig {
                discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
                client_id: "my-client".to_string(),
                client_secret_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "oidc-client".to_string(),
                    namespace: None,
                }),
                session_secret_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "oidc-session".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            },
        )))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::OpenidConnect(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        assert_eq!(
            config.resolved_oidc_client_secret.as_deref(),
            Some("client-secret-value")
        );
        assert_eq!(config.resolved_session_secret.as_deref(), Some("session-secret-value"));
        assert_eq!(ctx.secret_ref_manager().get_refs("default/oidc-client").len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("default/oidc-session").len(), 1);
    }

    #[tokio::test]
    async fn test_parse_resolves_jwe_secret_and_registers_refs() {
        let jwe_secret = build_secret("default", "jwe-secret", "secret", "0123456789abcdef0123456789abcdef");
        let mut secrets = HashMap::new();
        secrets.insert("default/jwe-secret".to_string(), jwe_secret);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("jwe-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::JweDecrypt(
            JweDecryptConfig {
                secret_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "jwe-secret".to_string(),
                    namespace: None,
                }),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::JweDecrypt(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        assert!(config.resolved_credential.is_some());
        let resolved_secret = config
            .resolved_credential
            .as_ref()
            .and_then(|c| c.secret.as_ref())
            .expect("expected resolved secret");
        assert!(!resolved_secret.is_empty());
        assert_eq!(ctx.secret_ref_manager().get_refs("default/jwe-secret").len(), 1);
    }

    #[tokio::test]
    async fn test_parse_resolves_hmac_credentials_and_registers_refs() {
        use std::io::{Read, Seek};
        use tracing::instrument::WithSubscriber;

        let credentials_yaml = r#"
- username: alice
  secret: alice-secret-32-bytes-0123456789abcd
  headers:
    X-Consumer-Username: alice
    X-Ignored: ignored
- username: bob
  secret: bob-secret-32-bytes-0123456789abcde
  headers:
    X-Consumer-Username: bob
"#;
        let hmac_secret = build_secret("default", "hmac-credentials", "credentials.yaml", credentials_yaml);
        let mut secrets = HashMap::new();
        secrets.insert("default/hmac-credentials".to_string(), hmac_secret);
        // Both malformed and truncated UTF-8 must be skipped before YAML parsing.
        let mut secret_refs = Vec::new();
        for (name, suffix) in [("invalid-hmac", &[0xff][..]), ("truncated-hmac", &[0xe2, 0x82][..])] {
            let mut secret = build_secret("default", name, "credentials.yaml", "private-credential-marker");
            secret
                .data
                .as_mut()
                .unwrap()
                .get_mut("credentials.yaml")
                .unwrap()
                .0
                .extend_from_slice(suffix);
            secrets.insert(format!("default/{name}"), secret);
            secret_refs.push(SecretObjectReference {
                group: None,
                kind: None,
                name: name.to_string(),
                namespace: None,
            });
        }
        secret_refs.push(SecretObjectReference {
            group: None,
            kind: None,
            name: "hmac-credentials".to_string(),
            namespace: None,
        });

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("hmac-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::HmacAuth(HmacAuthConfig {
            match_config: edgion_resources::resources::edgion_plugins::HmacMatchConfig {
                secret_groups: Some(vec![HmacAuthSecretGroup {
                    name: "default".to_string(),
                    description: None,
                    secret_refs,
                }]),
                ..Default::default()
            },
            upstream_header_fields: vec!["X-Consumer-Username".to_string()],
            ..Default::default()
        }))]);

        let mut log_file = tempfile::tempfile().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log_file.try_clone().unwrap())
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .finish();
        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).with_subscriber(subscriber).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        log_file.rewind().unwrap();
        let mut logs = String::new();
        log_file.read_to_string(&mut logs).unwrap();
        assert_eq!(logs.matches("Failed to decode credentials.yaml as UTF-8").count(), 2);
        assert!(logs.contains("secret_key=default/invalid-hmac"));
        assert!(logs.contains("secret_key=default/truncated-hmac"));
        assert!(!logs.contains("Failed to parse credentials.yaml"));
        assert!(!logs.contains("private-credential-marker"));
        for name in ["invalid-hmac", "truncated-hmac"] {
            assert_eq!(ctx.secret_ref_manager().get_refs(&format!("default/{name}")).len(), 1);
        }

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::HmacAuth(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        let resolved = config
            .resolved_credentials
            .as_ref()
            .expect("expected resolved credentials");
        assert_eq!(resolved.len(), 2);

        let alice = resolved.get("alice").expect("missing alice credential");
        assert_eq!(alice.secret, b"alice-secret-32-bytes-0123456789abcd".to_vec());
        assert_eq!(
            alice.headers.get("X-Consumer-Username").map(|v| v.as_str()),
            Some("alice")
        );
        assert!(!alice.headers.contains_key("X-Ignored"));

        assert_eq!(ctx.secret_ref_manager().get_refs("default/hmac-credentials").len(), 1);
    }

    #[tokio::test]
    async fn hmac_warnings_omit_credentials_and_preserve_first_wins() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, HmacMatchConfig};
        use std::io::{Read, Seek};
        use tracing::instrument::WithSubscriber;

        for as_condition in [false, true] {
            let username = "private-hmac-user@example.invalid";
            let credentials = [
                ("hmac-first", "first-secret"),
                ("hmac-same", "same-secret"),
                ("hmac-other", "other-secret"),
            ];
            let secrets = credentials
                .iter()
                .map(|(name, secret)| {
                    let yaml = format!(
                        "- username: {username}\n  secret: {secret}\n  headers:\n    X-Consumer-Username: {username}\n"
                    );
                    (
                        format!("default/{name}"),
                        build_secret("default", name, "credentials.yaml", &yaml),
                    )
                })
                .collect();
            let (ctx, _runtime) = make_ctx_with_secrets(secrets);
            let groups = [
                ("group-a", vec!["hmac-first", "hmac-same"]),
                ("group-b", vec!["hmac-other"]),
            ]
            .into_iter()
            .map(|(name, refs)| HmacAuthSecretGroup {
                name: name.to_string(),
                description: None,
                secret_refs: refs
                    .into_iter()
                    .map(|name| SecretObjectReference {
                        group: None,
                        kind: None,
                        name: name.to_string(),
                        namespace: None,
                    })
                    .collect(),
            })
            .collect();
            let match_config = HmacMatchConfig {
                secret_groups: Some(groups),
                min_secret_length: 32,
                ..Default::default()
            };
            let plugin = if as_condition {
                EdgionPlugin::RequestAccessPolicy(request_access_policy_config(
                    None,
                    Some(Condition::HmacAuth(match_config)),
                ))
            } else {
                EdgionPlugin::HmacAuth(HmacAuthConfig {
                    match_config,
                    upstream_header_fields: vec!["X-Consumer-Username".to_string()],
                    ..Default::default()
                })
            };
            let mut ep = EdgionPlugins::new("hmac-warning-plugins", EdgionPluginsSpec::default());
            ep.metadata.namespace = Some("default".to_string());
            ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(plugin)]);

            let mut log_file = tempfile::tempfile().unwrap();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(log_file.try_clone().unwrap())
                .with_max_level(tracing::Level::WARN)
                .without_time()
                .with_ansi(false)
                .finish();
            let parsed = match EdgionPluginsHandler.parse(ep, &ctx).with_subscriber(subscriber).await {
                ProcessResult::Continue(value) => value,
                ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
            };
            log_file.rewind().unwrap();
            let mut logs = String::new();
            log_file.read_to_string(&mut logs).unwrap();
            let warnings: Vec<_> = logs.lines().filter(|line| line.contains("HmacAuth:")).collect();
            assert_eq!(warnings.len(), 3);
            assert!(warnings[0].contains("secret length is below configured minimum"));
            assert!(warnings[0].contains("secret_key=default/hmac-first"));
            assert!(warnings[0].contains("group=group-a"));
            assert!(warnings[0].contains("secret_len=12"));
            assert!(warnings[0].contains("min_len=32"));
            assert!(warnings[1].contains("Duplicate username within group, skipping"));
            assert!(warnings[1].contains("secret_key=default/hmac-same"));
            assert!(warnings[1].contains("group=group-a"));
            assert!(warnings[2].contains("duplicate username across groups; first-wins kept"));
            assert!(warnings[2].contains("secret_key=default/hmac-other"));
            assert!(warnings[2].contains("existing_group=group-a"));
            assert!(warnings[2].contains("incoming_group=group-b"));
            for warning in warnings {
                assert!(warning.contains("edgion_plugins=EdgionPlugins/default/hmac-warning-plugins"));
            }
            assert!(!logs.contains(username));
            assert!(!logs.contains("username="));
            for (name, secret) in credentials {
                assert!(!logs.contains(secret));
                assert_eq!(ctx.secret_ref_manager().get_refs(&format!("default/{name}")).len(), 1);
            }

            let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
                EdgionPlugin::HmacAuth(config) => &config.match_config,
                EdgionPlugin::RequestAccessPolicy(config) => {
                    let Condition::HmacAuth(config) = first_request_access_policy_condition(config) else {
                        panic!("expected HMAC condition");
                    };
                    config
                }
                _ => panic!("unexpected plugin"),
            };
            let resolved = config.resolved_credentials.as_ref().unwrap();
            assert_eq!(resolved.len(), 1);
            assert_eq!(resolved[username].secret, b"first-secret");
            assert_eq!(resolved[username].group, "group-a");
            if as_condition {
                assert!(resolved[username].headers.is_empty());
            } else {
                assert_eq!(resolved[username].headers["X-Consumer-Username"], username);
            }
        }
    }

    #[tokio::test]
    async fn test_parse_resolves_header_cert_auth_ca_secrets_and_registers_refs() {
        let ca_secret = build_secret(
            "default",
            "header-cert-ca",
            "ca.crt",
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
        );
        let mut secrets = HashMap::new();
        secrets.insert("default/header-cert-ca".to_string(), ca_secret);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("header-cert-auth-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::HeaderCertAuth(
            HeaderCertAuthConfig {
                mode: CertSourceMode::Header,
                ca_secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "header-cert-ca".to_string(),
                    namespace: None,
                }],
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::HeaderCertAuth(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        let resolved = config
            .resolved_ca_secrets
            .as_ref()
            .expect("expected resolved ca secrets");
        assert_eq!(resolved.len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("default/header-cert-ca").len(), 1);
    }

    // ── Wasm fetch/OCI ref status resolution tests (contract-02) ──────────

    use crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        WasmAuthHeaderSecretRef, WasmConfig, WasmFetchSpec, WasmOciSpec, WasmSourceSpec,
    };

    fn build_secret_typed(namespace: &str, name: &str, type_: Option<&str>, data: &[(&str, &[u8])]) -> Secret {
        let mut bmap = BTreeMap::new();
        for (k, v) in data {
            bmap.insert(k.to_string(), ByteString(v.to_vec()));
        }
        Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(bmap),
            type_: type_.map(str::to_string),
            ..Default::default()
        }
    }

    fn wasm_ep_with_fetch_auth(ns: &str, secret_ns: Option<&str>, secret_name: &str, key: &str) -> EdgionPlugins {
        let mut ep = EdgionPlugins::new("wasm-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some(ns.to_string());
        let config = WasmConfig {
            source: WasmSourceSpec {
                url: Some("https://repo.example.com/mod.wasm".to_string()),
                fetch: Some(WasmFetchSpec {
                    auth_header_secret_ref: Some(WasmAuthHeaderSecretRef {
                        secret: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: secret_name.to_string(),
                            namespace: secret_ns.map(|s| s.to_string()),
                        },
                        key: key.to_string(),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::Wasm(config))]);
        ep
    }

    fn wasm_ep_with_oci(ns: &str, secret_ns: Option<&str>, secret_name: &str) -> EdgionPlugins {
        let mut ep = EdgionPlugins::new("wasm-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some(ns.to_string());
        let config = WasmConfig {
            source: WasmSourceSpec {
                oci: Some(WasmOciSpec {
                    image: "registry.example.com/wasm/mod:latest".to_string(),
                    pull_secret_ref: Some(SecretObjectReference {
                        group: None,
                        kind: None,
                        name: secret_name.to_string(),
                        namespace: secret_ns.map(|s| s.to_string()),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::Wasm(config))]);
        ep
    }

    #[test]
    fn test_wasm_auth_header_missing_key_reports_decode_error() {
        // Secret object present, but the configured key is absent → present-but-unusable.
        let secret = build_secret_typed("default", "wasm-auth", None, &[("other", b"x")]);
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::from([("default/wasm-auth".to_string(), secret)]));
        let ep = wasm_ep_with_fetch_auth("default", None, "wasm-auth", "authorization");

        let errors = super::resolve_wasm_refs(&ep, &ctx);
        assert_eq!(errors.len(), 1, "got {errors:?}");
        match &errors[0] {
            ResolvedRefsError::DecodeError { key, detail } => {
                assert_eq!(*key, "authHeaderSecretRef");
                assert!(detail.contains("authorization"), "detail: {detail}");
            }
            other => panic!("expected DecodeError, got {other:?}"),
        }
    }

    #[test]
    fn test_wasm_auth_header_usable_reports_no_error() {
        let secret = build_secret_typed("default", "wasm-auth", None, &[("authorization", b"Bearer tok")]);
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::from([("default/wasm-auth".to_string(), secret)]));
        let ep = wasm_ep_with_fetch_auth("default", None, "wasm-auth", "authorization");

        assert!(super::resolve_wasm_refs(&ep, &ctx).is_empty());
    }

    #[test]
    fn test_wasm_auth_header_missing_secret_defers_to_generic_path() {
        // Absent Secret object must NOT be double-reported here.
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::new());
        let ep = wasm_ep_with_fetch_auth("default", None, "wasm-auth", "authorization");

        assert!(super::resolve_wasm_refs(&ep, &ctx).is_empty());
    }

    #[test]
    fn test_wasm_pull_secret_wrong_type_reports_invalid_secret_type() {
        let secret = build_secret_typed("default", "pull", Some("Opaque"), &[(".dockerconfigjson", b"{}")]);
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::from([("default/pull".to_string(), secret)]));
        let ep = wasm_ep_with_oci("default", None, "pull");

        let errors = super::resolve_wasm_refs(&ep, &ctx);
        assert_eq!(errors.len(), 1, "got {errors:?}");
        match &errors[0] {
            ResolvedRefsError::InvalidSecretType { got, .. } => assert_eq!(got, "Opaque"),
            other => panic!("expected InvalidSecretType, got {other:?}"),
        }
    }

    #[test]
    fn test_wasm_pull_secret_missing_dockerconfig_key_reports_decode_error() {
        let secret = build_secret_typed(
            "default",
            "pull",
            Some("kubernetes.io/dockerconfigjson"),
            &[("wrong-key", b"{}")],
        );
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::from([("default/pull".to_string(), secret)]));
        let ep = wasm_ep_with_oci("default", None, "pull");

        let errors = super::resolve_wasm_refs(&ep, &ctx);
        assert_eq!(errors.len(), 1, "got {errors:?}");
        match &errors[0] {
            ResolvedRefsError::DecodeError { key, .. } => assert_eq!(*key, ".dockerconfigjson"),
            other => panic!("expected DecodeError, got {other:?}"),
        }
    }

    #[test]
    fn test_wasm_pull_secret_usable_reports_no_error() {
        let secret = build_secret_typed(
            "default",
            "pull",
            Some("kubernetes.io/dockerconfigjson"),
            &[(".dockerconfigjson", br#"{"auths":{}}"#)],
        );
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::from([("default/pull".to_string(), secret)]));
        let ep = wasm_ep_with_oci("default", None, "pull");

        assert!(super::resolve_wasm_refs(&ep, &ctx).is_empty());
    }

    #[test]
    fn test_wasm_cross_ns_grant_missing_reports_ref_not_permitted() {
        // Target Secret exists and is otherwise usable, but the cross-namespace
        // reference has no ReferenceGrant → the grant check short-circuits.
        let secret = build_secret_typed("other-ns", "wasm-auth", None, &[("authorization", b"Bearer tok")]);
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::from([("other-ns/wasm-auth".to_string(), secret)]));
        let ep = wasm_ep_with_fetch_auth("default", Some("other-ns"), "wasm-auth", "authorization");

        let errors = super::resolve_wasm_refs(&ep, &ctx);
        assert_eq!(errors.len(), 1, "got {errors:?}");
        match &errors[0] {
            ResolvedRefsError::RefNotPermitted {
                from_namespace,
                target_namespace,
                target_name,
                ..
            } => {
                assert_eq!(from_namespace, "default");
                assert_eq!(target_namespace, "other-ns");
                assert_eq!(target_name, "wasm-auth");
            }
            other => panic!("expected RefNotPermitted, got {other:?}"),
        }
    }

    fn ca_ref(kind: &str, name: &str, ns: Option<&str>) -> BackendTLSPolicyCACertificateRef {
        BackendTLSPolicyCACertificateRef {
            group: String::new(),
            kind: kind.to_string(),
            name: name.to_string(),
            namespace: ns.map(str::to_string),
        }
    }

    fn wasm_ep_with_fetch_tls(
        ns: &str,
        ca_refs: Vec<BackendTLSPolicyCACertificateRef>,
        client_cert: Option<SecretObjectReference>,
    ) -> EdgionPlugins {
        use edgion_resources::resources::common::{OutboundTlsLocal, OutboundTlsValidation, TlsConfig};
        let mut ep = EdgionPlugins::new("wasm-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some(ns.to_string());
        let tls = TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal {
                validation: (!ca_refs.is_empty()).then_some(OutboundTlsValidation {
                    ca_certificate_refs: Some(ca_refs),
                    ..Default::default()
                }),
                client_certificate_ref: client_cert,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = WasmConfig {
            source: WasmSourceSpec {
                url: Some("https://repo.example.com/mod.wasm".to_string()),
                fetch: Some(WasmFetchSpec {
                    tls: Some(tls),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::Wasm(config))]);
        ep
    }

    #[test]
    fn test_wasm_tls_ca_ref_cross_ns_grant_missing_reports_ref_not_permitted() {
        // Cross-namespace CA Secret ref with no ReferenceGrant: the resolver
        // leaves it out of resolved_ca_certificates, so status must not stay green.
        let secret = build_secret_typed("other-ns", "wasm-ca", None, &[("ca.crt", b"pem")]);
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::from([("other-ns/wasm-ca".to_string(), secret)]));
        let ep = wasm_ep_with_fetch_tls("default", vec![ca_ref("Secret", "wasm-ca", Some("other-ns"))], None);

        let errors = super::resolve_wasm_refs(&ep, &ctx);
        assert_eq!(errors.len(), 1, "got {errors:?}");
        match &errors[0] {
            ResolvedRefsError::RefNotPermitted {
                target_namespace,
                target_name,
                ..
            } => {
                assert_eq!(target_namespace, "other-ns");
                assert_eq!(target_name, "wasm-ca");
            }
            other => panic!("expected RefNotPermitted, got {other:?}"),
        }
    }

    #[test]
    fn test_wasm_tls_ca_configmap_cross_ns_grant_missing_reports_ref_not_permitted() {
        // Same gap for a ConfigMap-kind CA ref.
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::new());
        let ep = wasm_ep_with_fetch_tls(
            "default",
            vec![ca_ref("ConfigMap", "wasm-ca-cm", Some("other-ns"))],
            None,
        );

        let errors = super::resolve_wasm_refs(&ep, &ctx);
        assert_eq!(errors.len(), 1, "got {errors:?}");
        assert!(
            matches!(&errors[0], ResolvedRefsError::RefNotPermitted { target_name, .. } if target_name == "wasm-ca-cm")
        );
    }

    #[test]
    fn test_wasm_tls_client_cert_cross_ns_grant_missing_reports_ref_not_permitted() {
        let secret = build_secret_typed("other-ns", "wasm-cc", Some("kubernetes.io/tls"), &[("tls.crt", b"c")]);
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::from([("other-ns/wasm-cc".to_string(), secret)]));
        let cc = SecretObjectReference {
            group: None,
            kind: None,
            name: "wasm-cc".to_string(),
            namespace: Some("other-ns".to_string()),
        };
        let ep = wasm_ep_with_fetch_tls("default", vec![], Some(cc));

        let errors = super::resolve_wasm_refs(&ep, &ctx);
        assert_eq!(errors.len(), 1, "got {errors:?}");
        assert!(
            matches!(&errors[0], ResolvedRefsError::RefNotPermitted { target_name, .. } if target_name == "wasm-cc")
        );
    }

    #[test]
    fn test_wasm_tls_refs_same_ns_report_no_error() {
        // Same-namespace CA + client-cert refs are never grant-gated → no status error.
        let (ctx, _rt) = make_ctx_with_secrets(HashMap::new());
        let cc = SecretObjectReference {
            group: None,
            kind: None,
            name: "wasm-cc".to_string(),
            namespace: None,
        };
        let ep = wasm_ep_with_fetch_tls("default", vec![ca_ref("Secret", "wasm-ca", None)], Some(cc));

        assert!(super::resolve_wasm_refs(&ep, &ctx).is_empty());
    }

    // ── RegionRoute + overrideRef resolution tests ────────────────────────

    /// Helper: build an EdgionConfigData with namespace and name set.
    fn build_edgion_config_data(namespace: &str, name: &str, spec: EdgionConfigDataSpec) -> EdgionConfigData {
        let mut ed = EdgionConfigData::new(name, spec);
        ed.metadata.namespace = Some(namespace.to_string());
        ed
    }

    #[tokio::test]
    async fn test_validate_region_route_override_ref_ok() {
        // RegionRouteOverride EdgionConfigData exists → resolve_override_refs returns no error
        let override_ed = build_edgion_config_data(
            "default",
            "rr-override",
            EdgionConfigDataSpec {
                unknown_fields: Vec::new(),
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                    enable: None,
                    active: None,
                    regions: None,
                }),
            },
        );

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/rr-override".to_string(), override_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "rr-override".to_string(),
                    namespace: None,
                    ..Default::default()
                }),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ref_errors = super::resolve_override_refs(&parsed, &ctx);
        assert!(ref_errors.is_empty(), "expected no errors; got: {:?}", ref_errors);
    }

    #[tokio::test]
    async fn test_parse_registers_override_ref_for_requeue() {
        // Two RegionRoute plugins each with a different overrideRef.
        // Both must be registered in edgion_config_data_refs so that an update to
        // either EdgionConfigData triggers a requeue of this EdgionPlugins.
        let make_override_ed = |name: &str| {
            build_edgion_config_data(
                "default",
                name,
                EdgionConfigDataSpec {
                    enable: true,
                    active: None,
                    visibility: Default::default(),
                    unknown_fields: Vec::new(),
                    data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                        enable: None,
                        active: None,
                        regions: None,
                    }),
                },
            )
        };

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/override-a".to_string(), make_override_ed("override-a"));
        ed_map.insert("default/override-b".to_string(), make_override_ed("override-b"));
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("dual-rr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![
            RequestFilterEntry::new(EdgionPlugin::RegionRoute(RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "override-a".to_string(),
                    namespace: None,
                    ..Default::default()
                }),
                ..Default::default()
            })),
            RequestFilterEntry::new(EdgionPlugin::RegionRoute(RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "override-b".to_string(),
                    namespace: None,
                    ..Default::default()
                }),
                ..Default::default()
            })),
        ]);

        let handler = EdgionPluginsHandler::new();
        let _parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ep_key = "EdgionPlugins:default/dual-rr-ep";
        let override_a_refs = runtime.edgion_config_data_refs.get_refs("default/override-a");
        let override_b_refs = runtime.edgion_config_data_refs.get_refs("default/override-b");
        assert!(
            override_a_refs.contains(&ep_key.to_string()),
            "override-a must reference ep; got: {:?}",
            override_a_refs
        );
        assert!(
            override_b_refs.contains(&ep_key.to_string()),
            "override-b must reference ep; got: {:?}",
            override_b_refs
        );
    }

    #[tokio::test]
    async fn test_validate_region_route_override_ref_missing() {
        // Empty EdgionConfigData store → resolve_override_refs() reports the missing reference.
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "missing-override".to_string(),
                    namespace: None,
                    ..Default::default()
                }),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();

        // preparse() must NOT report missing EdgionConfigData as a structural error (it's a ref-resolution concern).
        let preparse_errors = handler.preparse(&mut ep, &ctx);
        assert!(
            preparse_errors.is_empty(),
            "preparse should not flag missing EdgionConfigData (handled by ResolvedRefs), got: {:?}",
            preparse_errors
        );

        // parse() must succeed — missing EdgionConfigData is a ref-resolution concern, not a parse error.
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        // resolve_override_refs() is the path that surfaces the missing EdgionConfigData.
        let ref_errors = super::resolve_override_refs(&parsed, &ctx);
        assert_eq!(ref_errors.len(), 1, "expected one EdgionConfigDataNotFound error");
        match &ref_errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { namespace, name, expected_type } => {
                assert_eq!(namespace, "default");
                assert_eq!(name, "missing-override");
                assert_eq!(expected_type, "RegionRouteOverride");
            }
            other => panic!("expected EdgionConfigDataNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_validate_region_route_override_ref_type_mismatch() {
        // EdgionConfigData exists but has wrong type → resolve_override_refs reports error.
        let wrong_type_ed = build_edgion_config_data(
            "default",
            "not-a-rr-override",
            EdgionConfigDataSpec {
                unknown_fields: Vec::new(),
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::KeyList(edgion_resources::resources::edgion_config_data::KeyListData {
                    match_mode: edgion_resources::resources::edgion_config_data::KeyMatchMode::default(),
                    items: vec![],
                }),
            },
        );

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/not-a-rr-override".to_string(), wrong_type_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "not-a-rr-override".to_string(),
                    namespace: None,
                    ..Default::default()
                }),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ref_errors = super::resolve_override_refs(&parsed, &ctx);
        assert_eq!(
            ref_errors.len(),
            1,
            "expected one EdgionConfigDataNotFound for type mismatch"
        );
        match &ref_errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { expected_type, .. } => {
                assert_eq!(expected_type, "RegionRouteOverride");
            }
            other => panic!("expected EdgionConfigDataNotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_region_route_invalid_overlay_keeps_base_accepted_and_recovers() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{HashCalcConfig, RegionDef};

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let base = RegionDef {
            name: "base".to_string(),
            hash_range: [0, 99],
            backend_endpoint: "base.example.test".to_string(),
            tls: true,
            failover_to: None,
        };
        let mut ep = EdgionPlugins::new("region-consumer", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                regions: vec![base.clone()],
                hash_calc: Some(HashCalcConfig {
                    modulo: 100,
                    algorithm: Default::default(),
                    write_back: Vec::new(),
                }),
                override_ref: Some(EdgionConfigDataRef {
                    name: "overlay".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ))]);
        // An overlay can be structurally valid yet exceed this consumer's
        // domain. Later replacement must clear its previous reference error.
        for (range, expected_status) in [([0, 999], "False"), ([0, 99], "True")] {
            let mut region = base.clone();
            region.hash_range = range;
            let overlay = build_edgion_config_data(
                "default",
                "overlay",
                EdgionConfigDataSpec {
                    enable: true,
                    active: None,
                    visibility: Default::default(),
                    unknown_fields: Vec::new(),
                    data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                        regions: Some(vec![region]),
                        enable: None,
                        active: None,
                    }),
                },
            );
            runtime
                .edgion_config_data_store
                .replace_all(HashMap::from([("default/overlay".to_string(), overlay)]));
            EdgionPluginsHandler.update_status(&mut ep, &ctx, &[]);
            let conditions = &ep.status.as_ref().unwrap().conditions;
            assert_eq!(
                conditions.iter().find(|c| c.type_ == "Accepted").unwrap().status,
                "True"
            );
            let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
            assert_eq!(resolved.status, expected_status);
            if expected_status == "False" {
                assert_eq!(resolved.reason, "InvalidEdgionConfigDataRef");
                assert!(resolved.message.contains("default/overlay"));
                assert!(resolved.message.contains("modulo"));
            }
        }
    }

    #[test]
    fn test_region_route_disabled_overlay_and_disabled_consumer() {
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ep = EdgionPlugins::new("region-consumer", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(
            RegionRouteConfig {
                override_ref: Some(EdgionConfigDataRef {
                    name: "overlay".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ))]);
        for (envelope_enable, variant_enable) in [(false, None), (true, Some(false))] {
            let overlay = build_edgion_config_data(
                "default",
                "overlay",
                EdgionConfigDataSpec {
                    enable: envelope_enable,
                    active: None,
                    visibility: Default::default(),
                    unknown_fields: Vec::new(),
                    data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                        enable: variant_enable,
                        active: None,
                        regions: None,
                    }),
                },
            );
            runtime
                .edgion_config_data_store
                .replace_all(HashMap::from([("default/overlay".to_string(), overlay)]));
            let errors = super::resolve_override_refs(&ep, &ctx);
            if envelope_enable {
                assert!(errors.is_empty(), "variant enable remains inert");
            } else {
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0].reason(), "InvalidEdgionConfigDataRef");
                assert!(errors[0].message().contains("disabled"));
            }
        }
        runtime.edgion_config_data_store.replace_all(HashMap::new());
        ep.spec.request_plugins.as_mut().unwrap()[0].enable = false;
        assert!(super::resolve_override_refs(&ep, &ctx).is_empty());
        let entry = &mut ep.spec.request_plugins.as_mut().unwrap()[0];
        entry.enable = true;
        entry.policy_action = Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass);
        assert!(super::resolve_override_refs(&ep, &ctx).is_empty());
    }

    // ── Canary + activeProfileRef resolution tests ──────────────────────────

    #[tokio::test]
    async fn test_resolve_canary_active_profile_refs_ok() {
        // Selector EdgionConfigData exists → resolve_canary_active_profile_refs returns no error.
        use edgion_resources::resources::edgion_config_data::Selector;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            CanaryConfig, DynUpstreamTarget, UpstreamFallback,
        };

        let selector_ed = build_edgion_config_data(
            "default",
            "canary-selector",
            EdgionConfigDataSpec {
                unknown_fields: Vec::new(),
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("canary".to_string()),
                    description: None,
                }),
            },
        );

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/canary-selector".to_string(), selector_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::Canary(CanaryConfig {
            enable: true,
            active_profile: "stable".to_string(),
            profiles: HashMap::from([
                (
                    "stable".to_string(),
                    DynUpstreamTarget {
                        name: "app-v1".to_string(),
                        namespace: None,
                    },
                ),
                (
                    "canary".to_string(),
                    DynUpstreamTarget {
                        name: "app-v2".to_string(),
                        namespace: None,
                    },
                ),
            ]),
            active_profile_ref: Some(EdgionConfigDataRef {
                name: "canary-selector".to_string(),
                namespace: None,
                ..Default::default()
            }),
            on_invalid: UpstreamFallback::Reject,
        }))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ref_errors = super::resolve_canary_active_profile_refs(&parsed, &ctx);
        assert!(ref_errors.is_empty(), "expected no ref errors, got: {:?}", ref_errors);
    }

    #[tokio::test]
    async fn test_resolve_canary_active_profile_refs_missing() {
        // Empty EdgionConfigData store → resolve_canary_active_profile_refs reports the missing reference.
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            CanaryConfig, DynUpstreamTarget, UpstreamFallback,
        };

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::Canary(CanaryConfig {
            enable: true,
            active_profile: "stable".to_string(),
            profiles: HashMap::from([(
                "stable".to_string(),
                DynUpstreamTarget {
                    name: "app-v1".to_string(),
                    namespace: None,
                },
            )]),
            active_profile_ref: Some(EdgionConfigDataRef {
                name: "canary-selector-absent".to_string(),
                namespace: None,
                ..Default::default()
            }),
            on_invalid: UpstreamFallback::Reject,
        }))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ref_errors = super::resolve_canary_active_profile_refs(&parsed, &ctx);
        assert_eq!(ref_errors.len(), 1, "expected one EdgionConfigDataNotFound error");
        match &ref_errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { namespace, name, expected_type } => {
                assert_eq!(namespace, "default");
                assert_eq!(name, "canary-selector-absent");
                assert_eq!(expected_type, "Selector");
            }
            other => panic!("expected EdgionConfigDataNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_resolve_canary_active_profile_refs_type_mismatch() {
        // EdgionConfigData exists but has wrong type → resolve_canary_active_profile_refs reports error.
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            CanaryConfig, DynUpstreamTarget, UpstreamFallback,
        };

        let wrong_type_ed = build_edgion_config_data(
            "default",
            "canary-selector-wrong-type",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::KeyList(edgion_resources::resources::edgion_config_data::KeyListData {
                    match_mode: edgion_resources::resources::edgion_config_data::KeyMatchMode::default(),
                    items: vec![],
                }),
            },
        );

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/canary-selector-wrong-type".to_string(), wrong_type_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::Canary(CanaryConfig {
            enable: true,
            active_profile: "stable".to_string(),
            profiles: HashMap::from([(
                "stable".to_string(),
                DynUpstreamTarget {
                    name: "app-v1".to_string(),
                    namespace: None,
                },
            )]),
            active_profile_ref: Some(EdgionConfigDataRef {
                name: "canary-selector-wrong-type".to_string(),
                namespace: None,
                ..Default::default()
            }),
            on_invalid: UpstreamFallback::Reject,
        }))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let ref_errors = super::resolve_canary_active_profile_refs(&parsed, &ctx);
        assert_eq!(
            ref_errors.len(),
            1,
            "expected one EdgionConfigDataNotFound for type mismatch"
        );
        match &ref_errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { expected_type, .. } => {
                assert_eq!(expected_type, "Selector");
            }
            other => panic!("expected EdgionConfigDataNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn cross_ns_canary_ref_registered_for_grant_revalidation() {
        // Verify Canary cross-namespace profile selectors use the same reference machinery.
        // (Task 6, Step 3): both the same-resource requeue loop and the cross_mgr loop in
        // parse() must register the ref, otherwise a ReferenceGrant change in the target
        // namespace would silently fail to requeue this EdgionPlugins.
        use edgion_resources::resources::edgion_config_data::Selector;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            CanaryConfig, DynUpstreamTarget, UpstreamFallback,
        };
        use edgion_resources::ResourceKind;

        let selector_ed = build_edgion_config_data(
            "shared-ns",
            "canary-selector",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("canary".to_string()),
                    description: None,
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("shared-ns/canary-selector".to_string(), selector_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let cfg = CanaryConfig {
            enable: true,
            active_profile: "stable".to_string(),
            profiles: HashMap::from([
                (
                    "stable".to_string(),
                    DynUpstreamTarget {
                        name: "app-v1".to_string(),
                        namespace: None,
                    },
                ),
                (
                    "canary".to_string(),
                    DynUpstreamTarget {
                        name: "app-v2".to_string(),
                        namespace: None,
                    },
                ),
            ]),
            active_profile_ref: Some(EdgionConfigDataRef {
                name: "canary-selector".to_string(),
                namespace: Some("shared-ns".to_string()),
                ..Default::default()
            }),
            on_invalid: UpstreamFallback::Reject,
        };

        let mut ep = EdgionPlugins::new("canary-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::Canary(cfg))]);

        let handler = EdgionPluginsHandler::new();
        let _ = handler.parse(ep, &ctx).await;

        // cross_ns_ref_mgr must contain this EP under "shared-ns" (the cross_mgr chain hookup).
        let refs = runtime.cross_ns_ref_mgr.get_refs("shared-ns");
        assert!(
            refs.iter().any(|rr| rr.kind == ResourceKind::EdgionPlugins),
            "Canary active_profile_ref cross-ns must register EP in cross_ns_ref_mgr; got: {:?}",
            refs
        );

        // edgion_config_data_refs must also track Canary (the same-resource requeue loop hookup).
        let ep_key = "EdgionPlugins:team-a/canary-ep";
        let cd_refs = runtime.edgion_config_data_refs.get_refs("shared-ns/canary-selector");
        assert!(
            cd_refs.contains(&ep_key.to_string()),
            "Canary active_profile_ref must be registered in edgion_config_data_refs; got: {:?}",
            cd_refs
        );
    }

    // ── IpRestriction + allowRefs/denyRefs resolution tests ─────────────────

    #[tokio::test]
    async fn ip_restriction_deny_refs_registered_and_resolved() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed an IpList EdgionConfigData.
        let ip_ed = build_edgion_config_data(
            "default",
            "blocklist",
            EdgionConfigDataSpec {
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
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/blocklist".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "blocklist".into(),
                    namespace: None,
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        // Registered for requeue.
        let refs = runtime.edgion_config_data_refs.get_refs("default/blocklist");
        assert!(
            refs.contains(&"EdgionPlugins:default/ipr-ep".to_string()),
            "got: {:?}",
            refs
        );
        // Resolves cleanly (IpList exists).
        assert!(super::resolve_ip_list_refs(&parsed, &ctx).is_empty());
    }

    #[tokio::test]
    async fn ip_restriction_missing_deny_ref_reports_iplist_error() {
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "missing".into(),
                    namespace: None,
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);
        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("skip: {}", reason),
        };
        let errors = super::resolve_ip_list_refs(&parsed, &ctx);
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { namespace, name, expected_type } => {
                assert_eq!(namespace, "default");
                assert_eq!(name, "missing");
                assert_eq!(expected_type, "IpList");
            }
            other => panic!("expected EdgionConfigDataNotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn ip_restriction_disabled_deny_ref_reports_error() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed a disabled (enable: false) IpList — should not count as resolved.
        let ip_ed = build_edgion_config_data(
            "default",
            "disabled-list",
            EdgionConfigDataSpec {
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
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/disabled-list".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "disabled-list".into(),
                    namespace: None,
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("skip: {}", reason),
        };
        let errors = super::resolve_ip_list_refs(&parsed, &ctx);
        assert_eq!(errors.len(), 1, "disabled IpList must not resolve; got: {:?}", errors);
        match &errors[0] {
            crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError::
                EdgionConfigDataNotFound { expected_type, .. } => {
                assert_eq!(expected_type, "IpList");
            }
            other => panic!("expected EdgionConfigDataNotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn ip_restriction_allow_ref_resolves() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed an enabled, valid IpList — should resolve cleanly.
        let ip_ed = build_edgion_config_data(
            "default",
            "allowlist",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "g".into(),
                        description: None,
                        cidrs: vec!["192.168.0.0/24".into()],
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/allowlist".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                allow_refs: Some(vec![EdgionConfigDataRef {
                    name: "allowlist".into(),
                    namespace: None,
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("skip: {}", reason),
        };
        // Enabled, valid IpList referenced via allow_refs -> resolves cleanly.
        assert!(
            super::resolve_ip_list_refs(&parsed, &ctx).is_empty(),
            "enabled valid IpList allow_ref must resolve without errors"
        );
    }

    // ── Inline IpRestriction entry-error collection tests ────────────────────

    #[test]
    fn collect_inline_ip_entry_errors_returns_error_for_bad_cidr() {
        use edgion_resources::resources::edgion_config_data::IpGroup;
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        let mut ep = EdgionPlugins::new("ipr-inline", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny: Some(vec![IpGroup {
                    name: "bad".to_string(),
                    description: None,
                    cidrs: vec!["not-a-cidr".to_string()],
                }]),
                ..Default::default()
            },
        ))]);

        let errs = super::collect_inline_ip_entry_errors(&ep);
        assert_eq!(errs.len(), 1, "expected one entry error for bad CIDR; got: {:?}", errs);
    }

    #[test]
    fn collect_inline_ip_entry_errors_empty_when_no_request_plugins() {
        let ep = EdgionPlugins::new("ipr-inline", EdgionPluginsSpec::default());
        let errs = super::collect_inline_ip_entry_errors(&ep);
        assert!(errs.is_empty(), "expected no errors when no plugins; got: {:?}", errs);
    }

    #[tokio::test]
    async fn ip_restriction_enabled_malformed_ip_list_resolves() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed an enabled IpList whose CIDRs produce a post-normalization duplicate
        // (10.0.0.0/8 and 10.0.0.1/8 both normalize to the same /8 network).
        // The gateway now uses build_grouped_matcher_partial and always inserts a usable
        // (partial/empty) matcher for an enabled IpList, so the ref IS considered resolved.
        // Per-entry problems surface on the IpList's own PartiallyInvalid status, not here.
        let ip_ed = build_edgion_config_data(
            "default",
            "malformed-list",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::IpList(IpListData {
                    items: vec![
                        IpGroup {
                            name: "a".into(),
                            description: None,
                            cidrs: vec!["10.0.0.0/8".into()],
                        },
                        IpGroup {
                            name: "b".into(),
                            description: None,
                            cidrs: vec!["10.0.0.1/8".into()],
                        },
                    ],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/malformed-list".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "malformed-list".into(),
                    namespace: None,
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("skip: {}", reason),
        };
        let errors = super::resolve_ip_list_refs(&parsed, &ctx);
        assert!(
            errors.is_empty(),
            "enabled IpList (even with malformed CIDRs) must resolve; got: {:?}",
            errors
        );
    }

    // ===== RequestAccessPolicy SecretRef resolution (restored; see Plan 3) =====

    // ── Task 6: cross_ns_ref_mgr registration tests ─────────────────────────

    #[tokio::test]
    async fn cross_ns_ref_registered_for_grant_revalidation() {
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;
        use edgion_resources::ResourceKind;

        // Build an EP in team-a with a cross-ns IpRestriction deny_ref into team-b.
        let ip_ed = build_edgion_config_data(
            "team-b",
            "blocklist",
            EdgionConfigDataSpec {
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
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("team-b/blocklist".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "blocklist".into(),
                    namespace: Some("team-b".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let _ = handler.parse(ep, &ctx).await;

        // After parse(), cross_ns_ref_mgr must contain this EP's ResourceRef under the
        // target namespace key "team-b" so a ReferenceGrant change there requeues it.
        let refs = runtime.cross_ns_ref_mgr.get_refs("team-b");
        assert!(
            refs.iter().any(|rr| rr.kind == ResourceKind::EdgionPlugins),
            "EP must be registered under team-b in cross_ns_ref_mgr; got: {:?}",
            refs
        );
    }

    #[tokio::test]
    async fn cross_ns_request_access_policy_ref_registered_for_grant_revalidation() {
        // Verify the policy active_profile_ref cross-namespace registration.
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, Selector};
        use edgion_resources::ResourceKind;

        let selector_ed = build_edgion_config_data(
            "shared-ns",
            "gac-selector",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("lockdown".to_string()),
                    description: None,
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("shared-ns/gac-selector".to_string(), selector_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let cfg = request_access_policy_config(
            Some(EdgionConfigDataRef {
                name: "gac-selector".into(),
                namespace: Some("shared-ns".into()),
                ..Default::default()
            }),
            None,
        );

        let mut ep = EdgionPlugins::new("gac-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(cfg))]);

        let handler = EdgionPluginsHandler::new();
        let _ = handler.parse(ep, &ctx).await;

        // cross_ns_ref_mgr must contain this EP under "shared-ns".
        let refs = runtime.cross_ns_ref_mgr.get_refs("shared-ns");
        assert!(
            refs.iter().any(|rr| rr.kind == ResourceKind::EdgionPlugins),
            "RequestAccessPolicy active_profile_ref must register EP in cross_ns_ref_mgr; got: {:?}",
            refs
        );

        // edgion_config_data_refs must also track RequestAccessPolicy.
        let ep_key = "EdgionPlugins:team-a/gac-ep";
        let cd_refs = runtime.edgion_config_data_refs.get_refs("shared-ns/gac-selector");
        assert!(
            cd_refs.contains(&ep_key.to_string()),
            "RequestAccessPolicy active_profile_ref must be registered in edgion_config_data_refs; got: {:?}",
            cd_refs
        );
    }

    // ── Cross-namespace ref-denial marking tests ────────────────────────────

    #[tokio::test]
    async fn cross_ns_deny_ref_without_grant_is_marked() {
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed: EdgionConfigData "team-b/blocklist" of type IpList (enabled, Namespace visibility).
        let ip_ed = build_edgion_config_data(
            "team-b",
            "blocklist",
            EdgionConfigDataSpec {
                unknown_fields: Vec::new(),
                enable: true,
                active: None,
                visibility: Default::default(), // Namespace — requires a grant for cross-ns
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "g".into(),
                        description: None,
                        cidrs: vec!["9.9.9.9/32".into()],
                    }],
                }),
            },
        );

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("team-b/blocklist".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);
        // No ReferenceGrant added -> cross-ns ref must be denied.

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "blocklist".into(),
                    namespace: Some("team-b".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(ep) => ep,
            _ => panic!("expected Continue"),
        };

        let r = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .and_then(|e| match &e.plugin {
                EdgionPlugin::IpRestriction(c) => c.deny_refs.as_ref()?.first(),
                _ => None,
            })
            .expect("missing deny_refs[0]");
        assert!(
            r.ref_denied.is_some(),
            "cross-ns ref without grant must be marked denied; got ref_denied = {:?}",
            r.ref_denied
        );
        let denied = r.ref_denied.as_ref().unwrap();
        assert_eq!(denied.target_namespace, "team-b");
        assert_eq!(denied.target_name, "blocklist");
        assert_eq!(denied.reason.as_deref(), Some("NoMatchingReferenceGrant"));
    }

    // ── Completeness guard: every EdgionPlugin variant must be classified ───
    #[test]
    fn ep_ref_bearing_variants_are_classified() {
        // Exhaustive match: a new ref-bearing variant forces an update here.
        // Only RegionRoute, IpRestriction, RequestAccessPolicy, and Canary carry
        // EdgionConfigData refs; everything else must map to false.
        fn carries_config_data_ref(p: &EdgionPlugin) -> bool {
            match p {
                EdgionPlugin::RegionRoute(_)
                | EdgionPlugin::IpRestriction(_)
                | EdgionPlugin::RequestAccessPolicy(_)
                | EdgionPlugin::Canary(_) => true,
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
                | EdgionPlugin::RateLimit(_)
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
                | EdgionPlugin::TraceContext(_)
                | EdgionPlugin::ExtProc(_)
                | EdgionPlugin::Guardrail(_)
                | EdgionPlugin::AiGuard(_)
                | EdgionPlugin::GeoIpLocation(_)
                | EdgionPlugin::Wasm(_) => false,
            }
        }
        let _ = carries_config_data_ref; // referenced for exhaustiveness guard
    }

    #[tokio::test]
    async fn request_access_policy_merges_ip_refs_and_tracks_missing_siblings() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessRuleType;

        let ip_data = build_edgion_config_data(
            "default",
            "shared-networks",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "shared".to_string(),
                        description: None,
                        cidrs: vec!["203.0.113.0/24".to_string()],
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime
            .edgion_config_data_store
            .replace_all(HashMap::from([("default/shared-networks".to_string(), ip_data)]));

        let mut config = request_access_policy_config(None, None);
        let rule = &mut config.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0];
        let RequestAccessRuleType::DirectPeerIp(ip) = &mut rule.rule else {
            panic!("expected directPeerIp rule")
        };
        ip.ip.ip_refs = vec![
            EdgionConfigDataRef {
                name: "shared-networks".to_string(),
                ..Default::default()
            },
            EdgionConfigDataRef {
                name: "missing-networks".to_string(),
                ..Default::default()
            },
        ];

        let mut ep = EdgionPlugins::new("policy", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(config))]);
        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        let EdgionPlugin::RequestAccessPolicy(config) = &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected RequestAccessPolicy")
        };
        let rule = &config.runtime_profiles()["lockdown"].required_rule_groups[0].any_of_rules[0];
        let RequestAccessRuleType::DirectPeerIp(ip) = &rule.rule else {
            panic!("expected directPeerIp rule")
        };
        let resolved = ip.ip.resolved_ips.as_ref().expect("controller-expanded IP union");
        assert!(resolved.contains(&"0.0.0.0/0".to_string()));
        assert!(resolved.contains(&"203.0.113.0/24".to_string()));
        assert!(config
            .resolution_warnings
            .iter()
            .any(|warning| warning.contains("missing-networks")));
        assert_eq!(
            runtime
                .edgion_config_data_refs
                .get_refs("default/shared-networks")
                .len(),
            1
        );
        assert_eq!(
            runtime
                .edgion_config_data_refs
                .get_refs("default/missing-networks")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn request_access_policy_denied_ip_ref_is_partial_not_unresolved() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessRuleType;

        let ip_data = build_edgion_config_data(
            "team-b",
            "private-networks",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "private".to_string(),
                        description: None,
                        cidrs: vec!["203.0.113.0/24".to_string()],
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime
            .edgion_config_data_store
            .replace_all(HashMap::from([("team-b/private-networks".to_string(), ip_data)]));
        let mut config = request_access_policy_config(None, None);
        let rule = &mut config.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules[0];
        let RequestAccessRuleType::DirectPeerIp(ip) = &mut rule.rule else {
            panic!("expected directPeerIp rule")
        };
        ip.ip.ip_refs.push(EdgionConfigDataRef {
            name: "private-networks".to_string(),
            namespace: Some("team-b".to_string()),
            ..Default::default()
        });

        let mut ep = EdgionPlugins::new("policy", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(config))]);
        let mut parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut parsed, &ctx, &[]);

        let conditions = &parsed.status.as_ref().unwrap().conditions;
        let resolved = conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        let partial = conditions
            .iter()
            .find(|condition| condition.type_ == "PartiallyInvalid")
            .unwrap();
        assert_eq!(resolved.status, "True");
        assert_eq!(partial.status, "True");
        assert!(partial.message.contains("not permitted"));
        assert_eq!(
            runtime
                .edgion_config_data_refs
                .get_refs("team-b/private-networks")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn request_access_policy_expands_url_data_and_tracks_dependency() {
        use edgion_resources::resources::edgion_config_data::RequestAccessUrlAllowListData;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AccessPathMatch, AccessPathMatchType, RequestAccessRule, RequestAccessRuleType,
            RequestAccessUrlAllowListRef, UrlAccessCandidate, UrlAccessRuleConfig,
        };

        let url_data = build_edgion_config_data(
            "default",
            "vendor-callbacks",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::RequestAccessUrlAllowList(RequestAccessUrlAllowListData {
                    items: vec![UrlAccessCandidate {
                        name: "vendor".to_string(),
                        hosts: vec!["callbacks.example.com".to_string()],
                        paths: vec![AccessPathMatch {
                            match_type: AccessPathMatchType::PathPrefix,
                            value: "/callbacks/:vendor".to_string(),
                        }],
                        conditions: None,
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime
            .edgion_config_data_store
            .replace_all(HashMap::from([("default/vendor-callbacks".to_string(), url_data)]));

        let mut config = request_access_policy_config(None, None);
        config.profiles.get_mut("lockdown").unwrap().required_rule_groups[0].any_of_rules = vec![RequestAccessRule {
            name: "vendor-url".to_string(),
            rule: RequestAccessRuleType::Url(UrlAccessRuleConfig {
                hosts: Vec::new(),
                paths: Vec::new(),
                conditions: None,
                config_refs: vec![RequestAccessUrlAllowListRef {
                    reference: EdgionConfigDataRef {
                        name: "vendor-callbacks".to_string(),
                        ..Default::default()
                    },
                    validate: Default::default(),
                }],
                resolved_candidates: None,
            }),
        }];

        let mut ep = EdgionPlugins::new("policy", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(config))]);
        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let EdgionPlugin::RequestAccessPolicy(config) = &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected RequestAccessPolicy")
        };
        let RequestAccessRuleType::Url(url) =
            &config.runtime_profiles()["lockdown"].required_rule_groups[0].any_of_rules[0].rule
        else {
            panic!("expected URL rule")
        };
        let candidates = url.resolved_candidates.as_ref().expect("expanded URL candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "vendor");
        assert_eq!(
            runtime
                .edgion_config_data_refs
                .get_refs("default/vendor-callbacks")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn request_access_policy_imported_url_candidate_binds_nested_secret() {
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, SecretMatchCondition};

        let condition = Condition::SecretMatch(SecretMatchCondition {
            negate: false,
            descriptor: Box::from(""),
            key: KeyGet::Header {
                name: "X-Vendor-Token".to_string(),
            },
            secret_key: "token".to_string(),
            secret_ref: SecretObjectReference {
                name: "vendor-token".to_string(),
                group: None,
                kind: None,
                namespace: None,
            },
            resolved_values: None,
        });
        let url_data = request_access_url_data(
            "default",
            "vendor-list",
            vec![request_access_url_candidate("vendor", vec![condition])],
        );
        let secret = build_secret("default", "vendor-token", "token", "nested-token-value");
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::from([("default/vendor-token".to_string(), secret)]));
        runtime
            .edgion_config_data_store
            .replace_all(HashMap::from([("default/vendor-list".to_string(), url_data)]));

        let authored = request_access_url_policy("                - name: vendor-list");
        let parsed = match EdgionPluginsHandler.parse(authored, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let candidates = first_resolved_url_candidates(&parsed);
        let Condition::SecretMatch(secret) = first_url_candidate_condition(&candidates[0]) else {
            panic!("expected nested Secret match")
        };
        assert_eq!(
            secret.resolved_values.as_deref(),
            Some(["nested-token-value".to_string()].as_slice())
        );
        assert_eq!(ctx.secret_ref_manager().get_refs("default/vendor-token").len(), 1);
        assert_eq!(
            runtime.edgion_config_data_refs.get_refs("default/vendor-list"),
            vec!["EdgionPlugins:default/url-policy".to_string()]
        );

        let EdgionPlugin::RequestAccessPolicy(config) = &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected policy")
        };
        use edgion_resources::resources::edgion_plugins::plugin_configs::RequestAccessRuleType;
        let RequestAccessRuleType::Url(authored_url) =
            &config.profiles["lockdown"].required_rule_groups[0].any_of_rules[0].rule
        else {
            panic!("expected authored URL rule")
        };
        assert!(authored_url.resolved_candidates.is_none());
    }

    #[tokio::test]
    async fn request_access_policy_imported_missing_secret_is_hard_and_recovers() {
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, SecretMatchCondition};

        let condition = Condition::SecretMatch(SecretMatchCondition {
            negate: false,
            descriptor: Box::from(""),
            key: KeyGet::Header {
                name: "X-Vendor-Token".to_string(),
            },
            secret_key: "token".to_string(),
            secret_ref: SecretObjectReference {
                name: "late-token".to_string(),
                group: None,
                kind: None,
                namespace: None,
            },
            resolved_values: Some(vec!["stale-token".to_string()]),
        });
        let url_data = request_access_url_data(
            "default",
            "vendor-list",
            vec![request_access_url_candidate("vendor", vec![condition])],
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime
            .edgion_config_data_store
            .replace_all(HashMap::from([("default/vendor-list".to_string(), url_data)]));
        let authored = request_access_url_policy("                - name: vendor-list");

        let mut missing = match EdgionPluginsHandler.parse(authored.clone(), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut missing, &ctx, &[]);
        let Condition::SecretMatch(secret) = first_url_candidate_condition(&first_resolved_url_candidates(&missing)[0])
        else {
            panic!("expected nested Secret")
        };
        assert!(secret.resolved_values.is_none());
        assert_eq!(ctx.secret_ref_manager().get_refs("default/late-token").len(), 1);
        let resolved = missing
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(resolved.status, "False");
        assert_eq!(resolved.reason, "SecretNotFound");

        runtime.secret_store.replace_all(HashMap::from([(
            "default/late-token".to_string(),
            build_secret("default", "late-token", "token", "fresh-token"),
        )]));
        let mut recovered = match EdgionPluginsHandler.parse(authored, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut recovered, &ctx, &[]);
        let Condition::SecretMatch(secret) =
            first_url_candidate_condition(&first_resolved_url_candidates(&recovered)[0])
        else {
            panic!("expected nested Secret")
        };
        assert_eq!(
            secret.resolved_values.as_deref(),
            Some(["fresh-token".to_string()].as_slice())
        );
        let resolved = recovered
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(resolved.status, "True");
    }

    #[tokio::test]
    async fn request_access_policy_cross_namespace_url_and_nested_secret_grants_are_independent() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, SecretMatchCondition};
        use edgion_resources::resources::{ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
        use edgion_resources::ResourceKind;

        fn grant(namespace: &str, name: &str, target_group: &str, target_kind: &str) -> ReferenceGrant {
            let mut grant = ReferenceGrant::new(
                name,
                ReferenceGrantSpec {
                    from: vec![ReferenceGrantFrom {
                        group: "edgion.io".to_string(),
                        kind: "EdgionPlugins".to_string(),
                        namespace: "tenant-a".to_string(),
                    }],
                    to: vec![ReferenceGrantTo {
                        group: target_group.to_string(),
                        kind: target_kind.to_string(),
                        name: None,
                    }],
                },
            );
            grant.metadata.namespace = Some(namespace.to_string());
            grant
        }

        fn authored_policy() -> EdgionPlugins {
            let mut policy = request_access_url_policy(
                "                - name: shared-urls\n                  namespace: shared-policy\n",
            );
            policy.metadata.namespace = Some("tenant-a".to_string());
            policy
        }

        let candidate = request_access_url_candidate(
            "shared-secret",
            vec![Condition::SecretMatch(SecretMatchCondition {
                negate: false,
                descriptor: Box::from(""),
                key: edgion_resources::common::KeyGet::Header {
                    name: "X-Origin-Token".to_string(),
                },
                secret_key: "token".to_string(),
                secret_ref: SecretObjectReference {
                    name: "origin-token".to_string(),
                    namespace: Some("shared-secret".to_string()),
                    group: None,
                    kind: None,
                },
                resolved_values: None,
            })],
        );
        let url_data = request_access_url_data("shared-policy", "shared-urls", vec![candidate]);
        let secret = build_secret("shared-secret", "origin-token", "token", "shared-token");
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::from([("shared-secret/origin-token".to_string(), secret)]));
        runtime
            .edgion_config_data_store
            .replace_all(HashMap::from([("shared-policy/shared-urls".to_string(), url_data)]));

        let mut no_grants = match EdgionPluginsHandler.parse(authored_policy(), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut no_grants, &ctx, &[]);
        assert!(first_resolved_url_candidates(&no_grants).is_empty());
        assert_eq!(
            no_grants
                .status
                .as_ref()
                .unwrap()
                .conditions
                .iter()
                .find(|condition| condition.type_ == "ResolvedRefs")
                .unwrap()
                .status,
            "True"
        );
        assert!(runtime
            .cross_ns_ref_mgr
            .get_refs("shared-policy")
            .iter()
            .any(|reference| { reference.kind == ResourceKind::EdgionPlugins && reference.name == "url-policy" }));
        assert!(runtime.cross_ns_ref_mgr.get_refs("shared-secret").is_empty());

        runtime.ref_grant_store.upsert(
            "shared-policy/allow-url".to_string(),
            Arc::new(grant("shared-policy", "allow-url", "edgion.io", "EdgionConfigData")),
        );
        let mut url_only = match EdgionPluginsHandler.parse(authored_policy(), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut url_only, &ctx, &[]);
        assert_eq!(first_resolved_url_candidates(&url_only).len(), 1);
        let Condition::SecretMatch(secret) =
            first_url_candidate_condition(&first_resolved_url_candidates(&url_only)[0])
        else {
            panic!("expected Secret match")
        };
        assert!(secret.resolved_values.is_none());
        let unresolved = url_only
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(unresolved.status, "False");
        assert_eq!(unresolved.reason, "RefNotPermitted");
        assert!(runtime
            .cross_ns_ref_mgr
            .get_refs("shared-secret")
            .iter()
            .any(|reference| { reference.kind == ResourceKind::EdgionPlugins && reference.name == "url-policy" }));

        runtime.ref_grant_store.upsert(
            "shared-secret/allow-secret".to_string(),
            Arc::new(grant("shared-secret", "allow-secret", "", "Secret")),
        );
        let mut both_grants = match EdgionPluginsHandler.parse(authored_policy(), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut both_grants, &ctx, &[]);
        let Condition::SecretMatch(secret) =
            first_url_candidate_condition(&first_resolved_url_candidates(&both_grants)[0])
        else {
            panic!("expected Secret match")
        };
        assert_eq!(
            secret.resolved_values.as_deref(),
            Some(["shared-token".to_string()].as_slice())
        );
        assert_eq!(
            both_grants
                .status
                .as_ref()
                .unwrap()
                .conditions
                .iter()
                .find(|condition| condition.type_ == "ResolvedRefs")
                .unwrap()
                .status,
            "True"
        );

        runtime.ref_grant_store.remove("shared-secret/allow-secret");
        let mut secret_grant_removed = match EdgionPluginsHandler.parse(authored_policy(), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut secret_grant_removed, &ctx, &[]);
        let condition = secret_grant_removed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(
            (condition.status.as_str(), condition.reason.as_str()),
            ("False", "RefNotPermitted")
        );
    }

    #[tokio::test]
    async fn request_access_policy_imported_url_candidate_expands_nested_ip_refs() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, IpAccessMatch};

        let ip_data = build_edgion_config_data(
            "default",
            "vendor-networks",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: vec![],
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "vendor".to_string(),
                        description: None,
                        cidrs: vec!["203.0.113.0/24".to_string()],
                    }],
                }),
            },
        );
        let condition = Condition::RemoteIpMatch(IpAccessMatch {
            ips: vec!["198.51.100.0/24".to_string()],
            ip_refs: vec![
                EdgionConfigDataRef {
                    name: "vendor-networks".to_string(),
                    ..Default::default()
                },
                EdgionConfigDataRef {
                    name: "missing-networks".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let url_data = request_access_url_data(
            "default",
            "vendor-list",
            vec![request_access_url_candidate("vendor", vec![condition])],
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime.edgion_config_data_store.replace_all(HashMap::from([
            ("default/vendor-list".to_string(), url_data),
            ("default/vendor-networks".to_string(), ip_data),
        ]));

        let parsed = match EdgionPluginsHandler
            .parse(request_access_url_policy("                - name: vendor-list"), &ctx)
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let Condition::RemoteIpMatch(ip) = first_url_candidate_condition(&first_resolved_url_candidates(&parsed)[0])
        else {
            panic!("expected nested remote IP match")
        };
        let resolved = ip.resolved_ips.as_ref().expect("resolved nested IP union");
        assert!(resolved.contains(&"198.51.100.0/24".to_string()));
        assert!(resolved.contains(&"203.0.113.0/24".to_string()));
        assert_eq!(runtime.edgion_config_data_refs.get_refs("default/vendor-list").len(), 1);
        assert_eq!(
            runtime
                .edgion_config_data_refs
                .get_refs("default/vendor-networks")
                .len(),
            1
        );
        assert_eq!(
            runtime
                .edgion_config_data_refs
                .get_refs("default/missing-networks")
                .len(),
            1
        );
        let EdgionPlugin::RequestAccessPolicy(config) = &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected policy")
        };
        assert!(config
            .resolution_warnings
            .iter()
            .any(|warning| warning.contains("missing-networks")));
    }

    #[tokio::test]
    async fn request_access_policy_url_ref_failures_are_partial_and_keep_valid_siblings() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};

        let valid = request_access_url_data(
            "default",
            "valid-list",
            vec![request_access_url_candidate("valid", vec![])],
        );
        let mut disabled = request_access_url_data(
            "default",
            "disabled-list",
            vec![request_access_url_candidate("disabled", vec![])],
        );
        disabled.spec.enable = false;
        let wrong_type = build_edgion_config_data(
            "default",
            "wrong-type",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: vec![],
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "wrong".to_string(),
                        description: None,
                        cidrs: vec!["127.0.0.0/8".to_string()],
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime.edgion_config_data_store.replace_all(HashMap::from([
            ("default/valid-list".to_string(), valid),
            ("default/disabled-list".to_string(), disabled),
            ("default/wrong-type".to_string(), wrong_type),
        ]));
        let refs = r#"
                - name: valid-list
                - name: missing-list
                - name: disabled-list
                - name: wrong-type
"#;
        let mut parsed = match EdgionPluginsHandler.parse(request_access_url_policy(refs), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut parsed, &ctx, &[]);
        assert_eq!(first_resolved_url_candidates(&parsed).len(), 1);
        for name in ["valid-list", "missing-list", "disabled-list", "wrong-type"] {
            assert_eq!(
                runtime
                    .edgion_config_data_refs
                    .get_refs(&format!("default/{name}"))
                    .len(),
                1
            );
        }
        let conditions = &parsed.status.as_ref().unwrap().conditions;
        assert_eq!(
            conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap()
                .status,
            "True"
        );
        assert_eq!(
            conditions
                .iter()
                .find(|condition| condition.type_ == "ResolvedRefs")
                .unwrap()
                .status,
            "True"
        );
        let partial = conditions
            .iter()
            .find(|condition| condition.type_ == "PartiallyInvalid")
            .unwrap();
        assert_eq!(partial.status, "True");
        for name in ["missing-list", "disabled-list", "wrong-type"] {
            assert!(partial.message.contains(name), "{}", partial.message);
        }
    }

    #[tokio::test]
    async fn request_access_policy_url_ref_limits_are_all_or_nothing() {
        let data = request_access_url_data(
            "default",
            "limited-list",
            vec![
                request_access_url_candidate("first", vec![]),
                request_access_url_candidate("second", vec![]),
            ],
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime
            .edgion_config_data_store
            .replace_all(HashMap::from([("default/limited-list".to_string(), data)]));

        let refs = r#"
                - name: limited-list
                  validate:
                    maxUrlRules: 1
"#;
        let rejected = match EdgionPluginsHandler.parse(request_access_url_policy(refs), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(
            first_resolved_url_candidates(&rejected).is_empty(),
            "an oversized reference must not import an order-dependent prefix"
        );
        assert!(super::request_access_policy_resolution_warnings(&rejected)[0].contains("validate limits"));

        let refs = r#"
                - name: limited-list
                  validate:
                    maxUrlRules: 2
"#;
        let accepted = match EdgionPluginsHandler.parse(request_access_url_policy(refs), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(first_resolved_url_candidates(&accepted).len(), 2);

        let mut hostless = request_access_url_candidate("hostless", vec![]);
        hostless.hosts.clear();
        runtime.edgion_config_data_store.replace_all(HashMap::from([(
            "default/limited-list".to_string(),
            request_access_url_data("default", "limited-list", vec![hostless]),
        )]));
        let default_rejected = match EdgionPluginsHandler
            .parse(
                request_access_url_policy("                - name: limited-list\n"),
                &ctx,
            )
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(first_resolved_url_candidates(&default_rejected).is_empty());
        let explicitly_allowed = match EdgionPluginsHandler
            .parse(
                request_access_url_policy(
                    "                - name: limited-list\n                  validate:\n                    allowNoHosts: true\n",
                ),
                &ctx,
            )
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(first_resolved_url_candidates(&explicitly_allowed).len(), 1);
    }

    #[tokio::test]
    async fn request_access_policy_url_secret_retarget_replaces_dependency_and_value() {
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, SecretMatchCondition};

        let secret_condition = |name: &str| {
            Condition::SecretMatch(SecretMatchCondition {
                negate: false,
                descriptor: Box::from(""),
                key: KeyGet::Header {
                    name: "X-Vendor-Token".to_string(),
                },
                secret_key: "token".to_string(),
                secret_ref: SecretObjectReference {
                    name: name.to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                },
                resolved_values: None,
            })
        };
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::from([
            (
                "default/token-a".to_string(),
                build_secret("default", "token-a", "token", "value-a"),
            ),
            (
                "default/token-b".to_string(),
                build_secret("default", "token-b", "token", "value-b"),
            ),
        ]));
        let authored = request_access_url_policy("                - name: vendor-list");
        runtime.edgion_config_data_store.replace_all(HashMap::from([(
            "default/vendor-list".to_string(),
            request_access_url_data(
                "default",
                "vendor-list",
                vec![request_access_url_candidate(
                    "vendor",
                    vec![secret_condition("token-a")],
                )],
            ),
        )]));
        let first = match EdgionPluginsHandler.parse(authored.clone(), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let Condition::SecretMatch(first_secret) =
            first_url_candidate_condition(&first_resolved_url_candidates(&first)[0])
        else {
            panic!("expected Secret match")
        };
        assert_eq!(
            first_secret.resolved_values.as_deref(),
            Some(["value-a".to_string()].as_slice())
        );

        runtime.edgion_config_data_store.replace_all(HashMap::from([(
            "default/vendor-list".to_string(),
            request_access_url_data(
                "default",
                "vendor-list",
                vec![request_access_url_candidate(
                    "vendor",
                    vec![secret_condition("token-b")],
                )],
            ),
        )]));
        let second = match EdgionPluginsHandler.parse(authored, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(ctx.secret_ref_manager().get_refs("default/token-a").is_empty());
        assert_eq!(ctx.secret_ref_manager().get_refs("default/token-b").len(), 1);
        let Condition::SecretMatch(second_secret) =
            first_url_candidate_condition(&first_resolved_url_candidates(&second)[0])
        else {
            panic!("expected Secret match")
        };
        assert_eq!(
            second_secret.resolved_values.as_deref(),
            Some(["value-b".to_string()].as_slice())
        );
    }

    #[tokio::test]
    async fn request_access_policy_url_ip_retarget_and_removal_clear_stale_dependencies() {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, IpAccessMatch};

        let ip_data = |name: &str, cidr: &str| {
            build_edgion_config_data(
                "default",
                name,
                EdgionConfigDataSpec {
                    enable: true,
                    active: None,
                    visibility: Default::default(),
                    unknown_fields: vec![],
                    data: ConfigEntry::IpList(IpListData {
                        items: vec![IpGroup {
                            name: name.to_string(),
                            description: None,
                            cidrs: vec![cidr.to_string()],
                        }],
                    }),
                },
            )
        };
        let condition = |name: &str| {
            Condition::DirectPeerIpMatch(IpAccessMatch {
                ips: vec!["127.0.0.1/32".to_string()],
                ip_refs: vec![EdgionConfigDataRef {
                    name: name.to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            })
        };
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let authored = request_access_url_policy("                - name: vendor-list");
        runtime.edgion_config_data_store.replace_all(HashMap::from([
            (
                "default/vendor-list".to_string(),
                request_access_url_data(
                    "default",
                    "vendor-list",
                    vec![request_access_url_candidate("vendor", vec![condition("ip-a")])],
                ),
            ),
            ("default/ip-a".to_string(), ip_data("ip-a", "192.0.2.0/24")),
            ("default/ip-b".to_string(), ip_data("ip-b", "203.0.113.0/24")),
        ]));
        let _ = EdgionPluginsHandler.parse(authored.clone(), &ctx).await;
        assert_eq!(runtime.edgion_config_data_refs.get_refs("default/ip-a").len(), 1);

        runtime.edgion_config_data_store.replace_all(HashMap::from([
            (
                "default/vendor-list".to_string(),
                request_access_url_data(
                    "default",
                    "vendor-list",
                    vec![request_access_url_candidate("vendor", vec![condition("ip-b")])],
                ),
            ),
            ("default/ip-a".to_string(), ip_data("ip-a", "192.0.2.0/24")),
            ("default/ip-b".to_string(), ip_data("ip-b", "203.0.113.0/24")),
        ]));
        let second = match EdgionPluginsHandler.parse(authored.clone(), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(runtime.edgion_config_data_refs.get_refs("default/ip-a").is_empty());
        assert_eq!(runtime.edgion_config_data_refs.get_refs("default/ip-b").len(), 1);
        let Condition::DirectPeerIpMatch(ip) =
            first_url_candidate_condition(&first_resolved_url_candidates(&second)[0])
        else {
            panic!("expected direct peer IP match")
        };
        assert!(!ip.resolved_ips.as_ref().unwrap().contains(&"192.0.2.0/24".to_string()));
        assert!(ip
            .resolved_ips
            .as_ref()
            .unwrap()
            .contains(&"203.0.113.0/24".to_string()));

        runtime.edgion_config_data_store.replace_all(HashMap::from([
            (
                "default/vendor-list".to_string(),
                request_access_url_data(
                    "default",
                    "vendor-list",
                    vec![request_access_url_candidate("vendor", vec![])],
                ),
            ),
            ("default/ip-a".to_string(), ip_data("ip-a", "192.0.2.0/24")),
            ("default/ip-b".to_string(), ip_data("ip-b", "203.0.113.0/24")),
        ]));
        let third = match EdgionPluginsHandler.parse(authored, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(runtime.edgion_config_data_refs.get_refs("default/ip-b").is_empty());
        assert!(first_resolved_url_candidates(&third)[0].conditions.is_none());
        assert_eq!(runtime.edgion_config_data_refs.get_refs("default/vendor-list").len(), 1);
    }

    #[tokio::test]
    async fn request_access_policy_imported_url_candidate_resolves_hmac_credentials() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, HmacMatchConfig};

        let condition = Condition::HmacAuth(HmacMatchConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "vendor".to_string(),
                description: None,
                secret_refs: vec![SecretObjectReference {
                    name: "vendor-hmac".to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                }],
            }]),
            ..Default::default()
        });
        let credentials = "- username: vendor\n  secret: vendor-secret-at-least-32-bytes-0001\n";
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::from([(
            "default/vendor-hmac".to_string(),
            build_secret("default", "vendor-hmac", "credentials.yaml", credentials),
        )]));
        runtime.edgion_config_data_store.replace_all(HashMap::from([(
            "default/vendor-list".to_string(),
            request_access_url_data(
                "default",
                "vendor-list",
                vec![request_access_url_candidate("vendor", vec![condition])],
            ),
        )]));
        let parsed = match EdgionPluginsHandler
            .parse(request_access_url_policy("                - name: vendor-list"), &ctx)
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let Condition::HmacAuth(hmac) = first_url_candidate_condition(&first_resolved_url_candidates(&parsed)[0])
        else {
            panic!("expected nested HMAC")
        };
        assert!(hmac
            .resolved_credentials
            .as_ref()
            .is_some_and(|credentials| credentials.contains_key("vendor")));
        assert_eq!(ctx.secret_ref_manager().get_refs("default/vendor-hmac").len(), 1);
        assert_eq!(runtime.edgion_config_data_refs.get_refs("default/vendor-list").len(), 1);
    }

    #[tokio::test]
    async fn request_access_policy_resolves_secret_values() {
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, SecretMatchCondition};

        let mut secrets = HashMap::new();
        secrets.insert(
            "default/cf-origin-secret".to_string(),
            build_secret_typed(
                "default",
                "cf-origin-secret",
                Some("Opaque"),
                &[("token", b"s3cr3t-TOKEN"), ("username", b"public-user")],
            ),
        );
        let (ctx, _rt) = make_ctx_with_secrets(secrets);

        let condition = Condition::SecretMatch(SecretMatchCondition {
            negate: false,
            descriptor: Box::from(""),
            key: KeyGet::Header {
                name: "X-CF-Secret".into(),
            },
            secret_key: "token".to_string(),
            secret_ref: SecretObjectReference {
                name: "cf-origin-secret".into(),
                group: None,
                kind: None,
                namespace: None,
            },
            resolved_values: None,
        });
        let cfg = request_access_policy_config(None, Some(condition));

        let mut ep = EdgionPlugins::new("origin-lockdown", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(cfg))]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::RequestAccessPolicy(c) => c,
            _ => panic!("unexpected plugin type"),
        };
        let condition = first_request_access_policy_condition(config);
        let Condition::SecretMatch(condition) = condition else {
            panic!("unexpected condition type")
        };
        let resolved = condition
            .resolved_values
            .as_ref()
            .expect("expected resolved secret values");
        assert_eq!(resolved, &["s3cr3t-TOKEN".to_string()]);
        assert!(!resolved.contains(&"public-user".to_string()));
        assert_eq!(ctx.secret_ref_manager().get_refs("default/cf-origin-secret").len(), 1);
    }

    #[tokio::test]
    async fn request_access_policy_unusable_selected_secret_key_clears_stale_and_sets_status_false() {
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, SecretMatchCondition};

        let cases: [(&str, &[(&str, &[u8])], &str); 3] = [
            ("missing", &[("username", b"public-user")], "MissingKeys"),
            ("non-utf8", &[("token", &[0xff, 0xfe])], "DecodeError"),
            ("blank", &[("token", b" \n\t")], "DecodeError"),
        ];
        for (case, data, expected_reason) in cases {
            let secret = build_secret_typed("default", "selected-secret", Some("Opaque"), data);
            let (ctx, _runtime) =
                make_ctx_with_secrets(HashMap::from([("default/selected-secret".to_string(), secret)]));
            let condition = Condition::SecretMatch(SecretMatchCondition {
                key: KeyGet::Header {
                    name: "X-Selected-Token".to_string(),
                },
                secret_key: "token".to_string(),
                secret_ref: SecretObjectReference {
                    name: "selected-secret".to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                },
                negate: false,
                resolved_values: Some(vec!["stale-sensitive-value".to_string()]),
                descriptor: Box::from(""),
            });
            let mut ep = EdgionPlugins::new("selected-key-policy", EdgionPluginsSpec::default());
            ep.metadata.namespace = Some("default".to_string());
            ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(
                request_access_policy_config(None, Some(condition)),
            ))]);

            let mut parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
                ProcessResult::Continue(value) => value,
                ProcessResult::Skip { reason } => panic!("unexpected skip for {case}: {reason}"),
            };
            let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
                EdgionPlugin::RequestAccessPolicy(config) => config,
                _ => panic!("unexpected plugin type"),
            };
            let Condition::SecretMatch(condition) = first_request_access_policy_condition(config) else {
                panic!("unexpected condition type")
            };
            assert!(condition.resolved_values.is_none(), "{case}: stale values survived");

            EdgionPluginsHandler.update_status(&mut parsed, &ctx, &[]);
            let status = parsed
                .status
                .as_ref()
                .unwrap()
                .conditions
                .iter()
                .find(|condition| condition.type_ == "ResolvedRefs")
                .expect("ResolvedRefs condition");
            assert_eq!(status.status, "False", "{case}: {status:?}");
            assert_eq!(status.reason, expected_reason, "{case}: {status:?}");
            assert!(status.message.contains("token"), "{case}: {}", status.message);
            assert!(
                !status.message.contains("stale-sensitive-value"),
                "{case}: {}",
                status.message
            );
            assert!(!status.message.contains("public-user"), "{case}: {}", status.message);
        }
    }

    #[tokio::test]
    async fn request_access_policy_hmac_ignores_credential_metadata() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, HmacMatchConfig};

        let credentials_yaml = r#"
- username: alice
  secret: alice-secret-32-bytes-0123456789abcd
  headers:
    X-Consumer-Username: alice
"#;
        let mut secrets = HashMap::new();
        secrets.insert(
            "default/hmac-credentials".to_string(),
            build_secret("default", "hmac-credentials", "credentials.yaml", credentials_yaml),
        );
        let (ctx, _runtime) = make_ctx_with_secrets(secrets);
        let condition = Condition::HmacAuth(HmacMatchConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "default".to_string(),
                description: None,
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "hmac-credentials".to_string(),
                    namespace: None,
                }],
            }]),
            ..Default::default()
        });
        let cfg = request_access_policy_config(None, Some(condition));

        let mut ep = EdgionPlugins::new("origin-lockdown", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(cfg))]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::RequestAccessPolicy(config) => config,
            _ => panic!("unexpected plugin type"),
        };
        let Condition::HmacAuth(config) = first_request_access_policy_condition(config) else {
            panic!("unexpected condition type")
        };
        let credential = config
            .resolved_credentials
            .as_ref()
            .and_then(|credentials| credentials.get("alice"))
            .expect("expected resolved credential");
        assert!(credential.headers.is_empty());
        assert_eq!(credential.secret, b"alice-secret-32-bytes-0123456789abcd");
    }

    #[tokio::test]
    async fn request_access_policy_hmac_reresolution_replaces_stale_credentials() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, HmacMatchConfig};

        let secret_ref = SecretObjectReference {
            group: None,
            kind: None,
            name: "hmac-credentials".to_string(),
            namespace: None,
        };
        let condition = Condition::HmacAuth(HmacMatchConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "default".to_string(),
                description: None,
                secret_refs: vec![secret_ref],
            }]),
            ..Default::default()
        });
        let cfg = request_access_policy_config(None, Some(condition));
        let mut ep = EdgionPlugins::new("origin-lockdown", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(cfg))]);

        let first_secret = build_secret(
            "default",
            "hmac-credentials",
            "credentials.yaml",
            "- username: alice\n  secret: alice-secret-32-bytes-0123456789abcd\n",
        );
        let (ctx, runtime) =
            make_ctx_with_secrets(HashMap::from([("default/hmac-credentials".to_string(), first_secret)]));
        let first = match EdgionPluginsHandler.parse(ep.clone(), &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let first_config = match &first.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::RequestAccessPolicy(config) => config,
            _ => panic!("unexpected plugin type"),
        };
        let Condition::HmacAuth(first_hmac) = first_request_access_policy_condition(first_config) else {
            panic!("unexpected condition type")
        };
        assert!(first_hmac
            .resolved_credentials
            .as_ref()
            .is_some_and(|credentials| credentials.contains_key("alice")));

        let rotated_secret = build_secret(
            "default",
            "hmac-credentials",
            "credentials.yaml",
            "- username: bob\n  secret: bob-secret-32-bytes-0123456789abcde\n",
        );
        runtime.secret_store.replace_all(HashMap::from([(
            "default/hmac-credentials".to_string(),
            rotated_secret,
        )]));
        let second = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let second_config = match &second.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::RequestAccessPolicy(config) => config,
            _ => panic!("unexpected plugin type"),
        };
        let Condition::HmacAuth(second_hmac) = first_request_access_policy_condition(second_config) else {
            panic!("unexpected condition type")
        };
        let credentials = second_hmac
            .resolved_credentials
            .as_ref()
            .expect("expected rotated credentials");
        assert!(!credentials.contains_key("alice"));
        assert_eq!(
            credentials.get("bob").map(|credential| credential.secret.as_slice()),
            Some(b"bob-secret-32-bytes-0123456789abcde".as_slice())
        );
    }

    #[tokio::test]
    async fn request_access_policy_no_secret_leaves_values_none() {
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, SecretMatchCondition};

        let (ctx, _rt) = make_ctx_with_secrets(HashMap::new());

        let condition = Condition::SecretMatch(SecretMatchCondition {
            negate: false,
            descriptor: Box::from(""),
            key: KeyGet::Header {
                name: "X-CF-Secret".into(),
            },
            secret_key: "token".to_string(),
            secret_ref: SecretObjectReference {
                name: "missing-secret".into(),
                group: None,
                kind: None,
                namespace: None,
            },
            resolved_values: None,
        });
        let cfg = request_access_policy_config(None, Some(condition));

        let mut ep = EdgionPlugins::new("p", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(cfg))]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::RequestAccessPolicy(c) => c,
            _ => panic!("unexpected plugin type"),
        };
        let condition = first_request_access_policy_condition(config);
        let Condition::SecretMatch(condition) = condition else {
            panic!("unexpected condition type")
        };
        assert!(condition.resolved_values.is_none());
    }

    // ── RefNotPermitted status reporting tests (Task 5) ─────────────────────

    #[tokio::test]
    async fn denied_ref_reports_ref_not_permitted_reason() {
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

        // Seed an IpList in team-b (cross-namespace target, right type) — no ReferenceGrant.
        let ip_ed = build_edgion_config_data(
            "team-b",
            "blocklist",
            EdgionConfigDataSpec {
                unknown_fields: Vec::new(),
                enable: true,
                active: None,
                visibility: Default::default(), // Namespace visibility -> cross-ns requires grant
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "g".into(),
                        description: None,
                        cidrs: vec!["9.9.9.9/32".into()],
                    }],
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("team-b/blocklist".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("ipr-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "blocklist".into(),
                    namespace: Some("team-b".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let mut parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        // parse() marks deny_refs[0].ref_denied = Some(...) (no grant, right-type target exists).

        handler.update_status(&mut parsed, &ctx, &[]);

        let cond = parsed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|c| c.type_ == "ResolvedRefs")
            .cloned()
            .expect("ResolvedRefs condition must be present");
        assert_eq!(cond.status, "False");
        assert_eq!(
            cond.reason, "RefNotPermitted",
            "denied cross-ns ref must yield reason RefNotPermitted"
        );
    }

    #[tokio::test]
    async fn request_access_policy_denied_active_profile_ref_reports_ref_not_permitted() {
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, Selector};

        // Seed a Selector EdgionConfigData in team-b (cross-namespace target, right type).
        let selector_ed = build_edgion_config_data(
            "team-b",
            "selector-cfg",
            EdgionConfigDataSpec {
                unknown_fields: Vec::new(),
                enable: true,
                active: None,
                visibility: Default::default(), // Namespace visibility -> cross-ns requires grant
                data: ConfigEntry::Selector(Selector {
                    active: Some("lockdown".to_string()),
                    description: None,
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("team-b/selector-cfg".to_string(), selector_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let cfg = request_access_policy_config(
            Some(EdgionConfigDataRef {
                name: "selector-cfg".into(),
                namespace: Some("team-b".into()),
                ..Default::default()
            }),
            None,
        );

        let mut ep = EdgionPlugins::new("gac-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(cfg))]);

        let handler = EdgionPluginsHandler::new();
        let mut parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        // parse() marks active_profile_ref.ref_denied = Some(...) (no grant, right-type target exists).

        handler.update_status(&mut parsed, &ctx, &[]);

        let cond = parsed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|c| c.type_ == "ResolvedRefs")
            .cloned()
            .expect("ResolvedRefs condition must be present");
        assert_eq!(cond.status, "False");
        assert_eq!(
            cond.reason, "RefNotPermitted",
            "denied RequestAccessPolicy active_profile_ref must yield reason RefNotPermitted"
        );
    }

    #[tokio::test]
    async fn disabled_selector_active_profile_ref_reports_not_found() {
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, Selector};

        // Seed a Selector EdgionConfigData with enable: false in the same namespace.
        // Same-namespace refs are never denied by ReferenceGrant logic, so ref_denied stays None.
        // A disabled Selector must fall to EdgionConfigDataNotFound (reason InvalidEdgionConfigDataRef),
        // not RefNotPermitted.
        let selector_ed = build_edgion_config_data(
            "default",
            "disabled-selector",
            EdgionConfigDataSpec {
                enable: false,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("lockdown".to_string()),
                    description: None,
                }),
            },
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("default/disabled-selector".to_string(), selector_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let cfg = request_access_policy_config(
            Some(EdgionConfigDataRef {
                name: "disabled-selector".into(),
                namespace: None,
                ..Default::default()
            }),
            None,
        );

        let mut ep = EdgionPlugins::new("gac-disabled-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(cfg))]);

        let handler = EdgionPluginsHandler::new();
        let mut parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        handler.update_status(&mut parsed, &ctx, &[]);

        let cond = parsed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|c| c.type_ == "ResolvedRefs")
            .cloned()
            .expect("ResolvedRefs condition must be present");
        assert_eq!(cond.status, "False", "disabled Selector must not resolve");
        assert_eq!(
            cond.reason, "InvalidEdgionConfigDataRef",
            "disabled same-ns Selector must yield InvalidEdgionConfigDataRef, not RefNotPermitted; got: {}",
            cond.reason
        );
    }

    #[tokio::test]
    async fn ep_on_delete_clears_cross_ns_refs() {
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, IpListData};
        use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;
        use edgion_resources::ResourceKind;

        // Seed a cross-ns IpList target in team-b so parse() registers the cross-ns ref.
        let ip_ed = build_edgion_config_data(
            "team-b",
            "blocklist",
            EdgionConfigDataSpec {
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
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ed_map = HashMap::new();
        ed_map.insert("team-b/blocklist".to_string(), ip_ed);
        runtime.edgion_config_data_store.replace_all(ed_map);

        let mut ep = EdgionPlugins::new("test-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::IpRestriction(
            IpRestrictionConfig {
                deny_refs: Some(vec![EdgionConfigDataRef {
                    name: "blocklist".into(),
                    namespace: Some("team-b".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        // parse() registers the EP under team-b in cross_ns_ref_mgr.
        let _ = handler.parse(ep.clone(), &ctx).await;

        let refs_before = runtime.cross_ns_ref_mgr.get_refs("team-b");
        assert!(
            refs_before.iter().any(|rr| rr.kind == ResourceKind::EdgionPlugins),
            "EP must be registered under team-b in cross_ns_ref_mgr after parse; got: {:?}",
            refs_before
        );

        // on_delete() must clear the registration.
        handler.on_delete(&ep, &ctx).await;

        let refs_after = runtime.cross_ns_ref_mgr.get_refs("team-b");
        assert!(
            !refs_after.iter().any(|rr| rr.kind == ResourceKind::EdgionPlugins),
            "EP must not be in cross_ns_ref_mgr under team-b after on_delete; got: {:?}",
            refs_after
        );
    }

    #[tokio::test]
    async fn request_access_policy_missing_secret_clears_stale_values() {
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, SecretMatchCondition};

        let (ctx, _rt) = make_ctx_with_secrets(HashMap::new());

        let condition = Condition::SecretMatch(SecretMatchCondition {
            negate: false,
            descriptor: Box::from(""),
            key: KeyGet::Header {
                name: "X-CF-Secret".into(),
            },
            secret_key: "token".to_string(),
            secret_ref: SecretObjectReference {
                name: "cf-origin-secret".into(),
                group: None,
                kind: None,
                namespace: None,
            },
            resolved_values: Some(vec!["OLD-TOKEN".into()]),
        });

        let cfg = request_access_policy_config(None, Some(condition));

        let mut ep = EdgionPlugins::new("p", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestAccessPolicy(cfg))]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::RequestAccessPolicy(c) => c,
            _ => panic!("unexpected plugin type"),
        };
        let condition = first_request_access_policy_condition(config);
        let Condition::SecretMatch(condition) = condition else {
            panic!("unexpected condition type")
        };
        assert!(condition.resolved_values.is_none());
    }

    #[tokio::test]
    async fn forward_auth_resolved_refs_missing() {
        use edgion_resources::resources::common::{
            CustomField, CustomSource, ExternalHttpConnection, OutboundRequestSpec, WebhookServiceRef, WebhookTarget,
        };
        use edgion_resources::resources::edgion_plugins::{ForwardAuthConfig, ForwardAuthDecision};

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        // ForwardAuth plugin with a custom header that embeds a ${secretRef:...} template.
        // The referenced Secret does not exist, so resolved_secrets stays None and
        // SecretRefManager registers the dependency. update_status then derives
        // ResolvedRefs=False / SecretNotFound from the dependency list.
        let mut request = OutboundRequestSpec::default();
        request.headers.forward_all = true;
        request.headers.custom = vec![CustomField {
            name: "X-Api-Key".to_string(),
            source: CustomSource::Template("${secretRef:default/missing:api-key}".to_string()),
        }];

        let cfg = ForwardAuthConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    service: WebhookServiceRef {
                        kind: None,
                        name: Some("auth-svc".to_string()),
                        namespace: None,
                        port: Some(8080),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            request,
            forward_body: false,
            decision: ForwardAuthDecision::default(),
            resolved_secrets: None,
        };

        let mut ep = EdgionPlugins::new("fa-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::ForwardAuth(cfg))]);

        let handler = EdgionPluginsHandler::new();
        let mut parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        handler.update_status(&mut parsed, &ctx, &[]);

        let cond = parsed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|c| c.type_ == "ResolvedRefs")
            .cloned()
            .expect("ResolvedRefs condition must be present");

        assert_eq!(
            cond.status, "False",
            "ResolvedRefs must be False when the ForwardAuth secretRef is missing"
        );
        assert_eq!(
            cond.reason, "SecretNotFound",
            "expected SecretNotFound reason; got: {}",
            cond.reason
        );
        assert!(
            cond.message.contains("missing"),
            "condition message must mention the missing secret name; got: {}",
            cond.message
        );
    }

    /// Verify that `resolve_ext_proc` populates `resolved_ca_secret` from
    /// `grpcService.outbound.validation.caCertificateRefs` when the referenced Secret is present
    /// in the store.
    #[tokio::test]
    async fn extproc_resolver_fills_resolved_secrets() {
        use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};
        use edgion_resources::resources::edgion_plugins::plugin_configs::GrpcServiceConfig;

        let ca_secret = build_secret("default", "extproc-ca", "ca.crt", "CA-PEM-DATA");
        let mut secrets = HashMap::new();
        secrets.insert("default/extproc-ca".to_string(), ca_secret);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("extproc-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::ExtProc(
            edgion_resources::resources::edgion_plugins::ExtProcConfig {
                grpc_service: GrpcServiceConfig {
                    target: "ext-proc.default.svc.cluster.local:9000".to_string(),
                    timeout_ms: 200,
                    tls: TlsConfig {
                        enabled: true,
                        outbound: OutboundTlsLocal {
                            validation: Some(edgion_resources::resources::common::OutboundTlsValidation {
                                ca_certificate_refs: Some(vec![
                                    edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef {
                                        kind: "Secret".to_string(),
                                        name: "extproc-ca".to_string(),
                                        ..Default::default()
                                    },
                                ]),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                },
                ..Default::default()
            },
        ))]);

        let handler = EdgionPluginsHandler::new();
        let parsed = match handler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };

        let entry = parsed
            .spec
            .request_plugins
            .as_ref()
            .and_then(|v| v.first())
            .expect("missing request plugin");
        let config = match &entry.plugin {
            EdgionPlugin::ExtProc(c) => c,
            _ => panic!("unexpected plugin type"),
        };

        assert!(
            config
                .grpc_service
                .tls
                .resolved_ca_certificates
                .as_ref()
                .is_some_and(|v| !v.is_empty()),
            "resolved_ca_certificates must be populated from caCertificateRefs"
        );
        assert!(
            config.grpc_service.tls.resolved_client_certificate.is_none(),
            "no clientCertificateRef was set"
        );
        assert_eq!(ctx.secret_ref_manager().get_refs("default/extproc-ca").len(), 1);
    }

    #[tokio::test]
    async fn guardrail_resolver_fills_request_and_tls_secrets() {
        use edgion_resources::resources::common::{
            CustomField, CustomSource, ExternalHttpConnection, OutboundRequestSpec, OutboundTlsLocal, PathSpec,
            TlsConfig, WebhookTarget,
        };
        use edgion_resources::resources::edgion_plugins::GuardrailConfig;

        let mut secrets = HashMap::new();
        secrets.insert(
            "default/guard-token".to_string(),
            build_secret("default", "guard-token", "token", "TOKEN"),
        );
        secrets.insert(
            "default/guard-ca".to_string(),
            build_secret("default", "guard-ca", "ca.crt", "CA-PEM"),
        );
        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut request = OutboundRequestSpec {
            path: Some(PathSpec {
                template: "/v1/decision".to_string(),
                allow_override: false,
            }),
            ..Default::default()
        };
        request.headers.custom.push(CustomField {
            name: "Authorization".to_string(),
            source: CustomSource::Template("Bearer ${secretRef:default/guard-token:token}".to_string()),
        });
        let config = GuardrailConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    url: Some("https://guard.example".to_string()),
                    ..Default::default()
                },
                tls: Some(TlsConfig {
                    enabled: true,
                    outbound: OutboundTlsLocal {
                        validation: Some(edgion_resources::resources::common::OutboundTlsValidation {
                            ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                                kind: "Secret".to_string(),
                                name: "guard-ca".to_string(),
                                ..Default::default()
                            }]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Default::default()
            },
            request,
            ..Default::default()
        };

        let mut ep = EdgionPlugins::new("guardrail-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::Guardrail(config))]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::Guardrail(config) => config,
            _ => panic!("unexpected plugin type"),
        };
        assert_eq!(
            config
                .resolved_secrets
                .as_ref()
                .and_then(|secrets| secrets.0.get("default/guard-token:token"))
                .map(String::as_str),
            Some("TOKEN")
        );
        assert!(config
            .conn
            .tls
            .as_ref()
            .and_then(|tls| tls.resolved_ca_certificates.as_ref())
            .is_some_and(|certificates| !certificates.is_empty()));
        assert_eq!(ctx.secret_ref_manager().get_refs("default/guard-token").len(), 1);
        assert_eq!(ctx.secret_ref_manager().get_refs("default/guard-ca").len(), 1);
    }

    fn guardrail_with_template_secret(secret_ref: &str) -> EdgionPlugin {
        use edgion_resources::resources::common::{
            CustomField, CustomSource, ExternalHttpConnection, OutboundRequestSpec, PathSpec, WebhookTarget,
        };
        use edgion_resources::resources::edgion_plugins::GuardrailConfig;

        let mut request = OutboundRequestSpec {
            path: Some(PathSpec {
                template: "/v1/decision".to_string(),
                allow_override: false,
            }),
            ..Default::default()
        };
        request.headers.custom.push(CustomField {
            name: "Authorization".to_string(),
            source: CustomSource::Template(format!("Bearer ${{secretRef:{secret_ref}}}")),
        });
        EdgionPlugin::Guardrail(GuardrailConfig {
            conn: ExternalHttpConnection {
                target: WebhookTarget {
                    url: Some("https://guard.example".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            request,
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn guardrail_present_secret_missing_key_sets_decode_error_status() {
        let secret = build_secret("default", "guard-token", "other", "value");
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::from([("default/guard-token".to_string(), secret)]));
        let mut ep = EdgionPlugins::new("guardrail-status", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(guardrail_with_template_secret(
            "default/guard-token:token",
        ))]);

        let mut parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut parsed, &ctx, &[]);
        let condition = parsed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(condition.status, "False");
        assert_eq!(condition.reason, "DecodeError");
        assert!(condition.message.contains("token"), "message: {}", condition.message);
    }

    #[tokio::test]
    async fn guardrail_cross_namespace_denial_sets_ref_not_permitted_status() {
        let secret = build_secret("policy", "guard-token", "token", "value");
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::from([("policy/guard-token".to_string(), secret)]));
        let mut ep = EdgionPlugins::new("guardrail-status", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("application".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(guardrail_with_template_secret(
            "policy/guard-token:token",
        ))]);

        let mut parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut parsed, &ctx, &[]);
        let condition = parsed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(condition.status, "False");
        assert_eq!(condition.reason, "RefNotPermitted");
        assert!(
            condition.message.contains("policy/guard-token"),
            "message: {}",
            condition.message
        );
    }

    #[tokio::test]
    async fn guardrail_missing_secret_is_reported_once() {
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());
        let mut ep = EdgionPlugins::new("guardrail-status", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(guardrail_with_template_secret(
            "default/guard-token:token",
        ))]);

        let mut parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        EdgionPluginsHandler.update_status(&mut parsed, &ctx, &[]);
        let condition = parsed
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .unwrap();
        assert_eq!(condition.status, "False");
        assert_eq!(condition.reason, "SecretNotFound");
        assert_eq!(condition.message.matches("default/guard-token").count(), 1);
    }

    // ── Phase 1 SAN guard tests ───────────────────────────────────────────────

    /// OpenidConnect plugin with `tls.validation.subjectAltNames` is accepted in Phase 2.
    /// SAN pinning is now enforced by the OIDC TLS verifier (use_preconfigured_tls + OutboundSanVerifier).
    #[tokio::test]
    async fn oidc_with_san_accepted_phase2() {
        use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
        use edgion_resources::resources::common::OutboundTlsValidation;

        let client_secret = build_secret("default", "oidc-client", "clientSecret", "client-secret-value");
        let session_secret = build_secret("default", "oidc-session", "sessionSecret", "session-secret-value");
        let mut secrets = HashMap::new();
        secrets.insert("default/oidc-client".to_string(), client_secret);
        secrets.insert("default/oidc-session".to_string(), session_secret);

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("oidc-san-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::OpenidConnect(Box::new(
            OpenidConnectConfig {
                discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
                client_id: "my-client".to_string(),
                client_secret_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "oidc-client".to_string(),
                    namespace: None,
                }),
                session_secret_ref: Some(SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "oidc-session".to_string(),
                    namespace: None,
                }),
                tls: edgion_resources::resources::common::OutboundTlsLocal {
                    validation: Some(OutboundTlsValidation {
                        subject_alt_names: Some(vec![SubjectAltName {
                            san_type: SubjectAltNameType::Hostname,
                            hostname: Some("idp.example.com".to_string()),
                            uri: None,
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
        )))]);

        let handler = EdgionPluginsHandler::new();
        let errors = handler.preparse(&mut ep, &ctx);

        assert!(
            errors.is_empty(),
            "OIDC with subjectAltNames should be accepted in Phase 2; got errors: {:?}",
            errors
        );
    }

    /// LdapAuth plugin with `tls.validation.subjectAltNames` is accepted in Phase 2.
    /// SAN pinning is now enforced by the LDAP TLS verifier (OutboundSanVerifier wrap).
    #[tokio::test]
    async fn ldap_with_san_accepted_phase2() {
        use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
        use edgion_resources::resources::common::OutboundTlsValidation;
        use edgion_resources::resources::edgion_plugins::LdapAuthConfig;

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("ldap-san-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::LdapAuth(LdapAuthConfig {
            ldap_host: "ldap.example.com".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            attribute: "uid".to_string(),
            tls: edgion_resources::resources::common::OutboundTlsLocal {
                validation: Some(OutboundTlsValidation {
                    subject_alt_names: Some(vec![SubjectAltName {
                        san_type: SubjectAltNameType::Hostname,
                        hostname: Some("ldap.example.com".to_string()),
                        uri: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }))]);

        let handler = EdgionPluginsHandler::new();
        let errors = handler.preparse(&mut ep, &ctx);

        assert!(
            errors.is_empty(),
            "LdapAuth with subjectAltNames should be accepted in Phase 2; got errors: {:?}",
            errors
        );
    }

    /// `tls.validation.hostname` (SNI override) is implemented on no outbound
    /// provider, so it must be rejected even on OIDC/LDAP where subjectAltNames
    /// IS enforced — otherwise it would be a silent no-op.
    #[tokio::test]
    async fn oidc_with_hostname_rejected() {
        use edgion_resources::resources::common::OutboundTlsValidation;

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("oidc-hostname-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::OpenidConnect(Box::new(
            OpenidConnectConfig {
                discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
                client_id: "my-client".to_string(),
                tls: edgion_resources::resources::common::OutboundTlsLocal {
                    validation: Some(OutboundTlsValidation {
                        hostname: Some("idp.example.com".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
        )))]);

        let handler = EdgionPluginsHandler::new();
        let errors = handler.preparse(&mut ep, &ctx);

        assert!(
            errors.iter().any(|e| e.contains("hostname")),
            "OIDC validation.hostname must be rejected as unimplemented; got: {:?}",
            errors
        );
    }

    /// ExtProc plugin with `grpcService.tls.validation.subjectAltNames` must
    /// produce an Accepted=False preparse error in Phase 1.
    #[tokio::test]
    async fn extproc_with_san_rejected_phase1() {
        use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
        use edgion_resources::resources::common::{OutboundTlsLocal, OutboundTlsValidation, TlsConfig};
        use edgion_resources::resources::edgion_plugins::{plugin_configs::GrpcServiceConfig, ExtProcConfig};

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("extproc-san-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::ExtProc(ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target: "ext-proc.default.svc.cluster.local:9000".to_string(),
                timeout_ms: 200,
                tls: TlsConfig {
                    enabled: true,
                    outbound: OutboundTlsLocal {
                        validation: Some(OutboundTlsValidation {
                            subject_alt_names: Some(vec![SubjectAltName {
                                san_type: SubjectAltNameType::Hostname,
                                hostname: Some("ext-proc.example.com".to_string()),
                                uri: None,
                            }]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
            ..Default::default()
        }))]);

        let handler = EdgionPluginsHandler::new();
        let errors = handler.preparse(&mut ep, &ctx);

        assert!(
            !errors.is_empty(),
            "expected preparse error for ExtProc subjectAltNames in Phase 1"
        );
        assert!(
            errors.iter().any(|e| e.contains("ExtProc")),
            "error must identify the provider; got: {:?}",
            errors
        );
    }

    /// A plugin without SAN pinning (only CA refs) must NOT be rejected.
    #[tokio::test]
    async fn oidc_ca_only_no_san_accepted() {
        use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
        use edgion_resources::resources::common::{OutboundTlsLocal, OutboundTlsValidation};

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("oidc-caonly-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::OpenidConnect(Box::new(
            OpenidConnectConfig {
                discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
                client_id: "my-client".to_string(),
                tls: OutboundTlsLocal {
                    validation: Some(OutboundTlsValidation {
                        ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                            kind: "Secret".to_string(),
                            name: "idp-ca".to_string(),
                            namespace: None,
                            ..Default::default()
                        }]),
                        subject_alt_names: None,
                        hostname: None,
                        well_known_ca_certificates: None,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
        )))]);

        let handler = EdgionPluginsHandler::new();
        let errors = handler.preparse(&mut ep, &ctx);

        // CA-only validation must not trigger the SAN guard.
        let san_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.contains("SAN pinning") || e.contains("subjectAltNames"))
            .collect();
        assert!(
            san_errors.is_empty(),
            "CA-only validation must not be rejected by the SAN guard; got: {:?}",
            errors
        );
    }

    // ── PluginPolicyRestricted condition removal (Task 3) ───────────────────

    #[test]
    fn update_status_removes_stale_policy_condition_when_no_hits() {
        // Simulates the K8s watch echo: object arrives with a live
        // PluginPolicyRestricted condition but the (absent) policy stamps
        // nothing. update_status must remove the stale condition.
        use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
            condition_reasons, condition_true, condition_types,
        };
        use edgion_resources::resources::edgion_plugins::EdgionPluginsStatus;

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let mut ep = EdgionPlugins::new("policy-restricted-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.status = Some(EdgionPluginsStatus {
            conditions: vec![condition_true(
                condition_types::PLUGIN_POLICY_RESTRICTED,
                condition_reasons::RESTRICTED_BY_POLICY,
                "stale",
                Some(1),
            )],
        });

        let handler = EdgionPluginsHandler;
        handler.update_status(&mut ep, &ctx, &[]);

        assert!(ep
            .status
            .unwrap()
            .conditions
            .iter()
            .all(|c| c.type_ != condition_types::PLUGIN_POLICY_RESTRICTED));
    }

    // ── UnknownConfigFields condition ───────────────────────────────────────

    #[test]
    fn update_status_sets_unknown_config_fields_condition_with_exact_paths() {
        use crate::conf_mgr::sync_runtime::resource_processor::status_utils::condition_types;

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        let spec = EdgionPluginsSpec {
            unknown_fields: vec!["config.denys".to_string(), "requestPlugins[0].config.typo".to_string()],
            ..EdgionPluginsSpec::default()
        };
        let mut ep = EdgionPlugins::new("typo-ep", spec);
        ep.metadata.namespace = Some("default".to_string());

        let handler = EdgionPluginsHandler;
        handler.update_status(&mut ep, &ctx, &[]);

        let status = ep.status.unwrap();
        let cond = status
            .conditions
            .iter()
            .find(|c| c.type_ == condition_types::UNKNOWN_CONFIG_FIELDS)
            .expect("UnknownConfigFields condition must be present");
        assert_eq!(cond.status, "True");
        assert_eq!(cond.message, "config.denys, requestPlugins[0].config.typo");

        // Accepted must stay True and untouched by this advisory condition
        // (validation_errors is empty here).
        let accepted = status
            .conditions
            .iter()
            .find(|c| c.type_ == condition_types::ACCEPTED)
            .expect("Accepted condition must be present");
        assert_eq!(accepted.status, "True");
    }

    #[test]
    fn update_status_removes_unknown_config_fields_condition_when_fixed() {
        // Simulates the operator fixing the typo on the next generation:
        // update_status must REMOVE the stale condition, not leave it
        // upserted (the wedge trap the stamp module's doc comment warns
        // about).
        use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
            condition_reasons, condition_true, condition_types,
        };
        use edgion_resources::resources::edgion_plugins::EdgionPluginsStatus;

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());

        // spec.unknown_fields is empty (fixed), but a live condition is
        // still on the resource from a previous generation.
        let mut ep = EdgionPlugins::new("fixed-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.status = Some(EdgionPluginsStatus {
            conditions: vec![condition_true(
                condition_types::UNKNOWN_CONFIG_FIELDS,
                condition_reasons::UNKNOWN_FIELDS_DETECTED,
                "stale",
                Some(1),
            )],
        });

        let handler = EdgionPluginsHandler;
        handler.update_status(&mut ep, &ctx, &[]);

        assert!(ep
            .status
            .unwrap()
            .conditions
            .iter()
            .all(|c| c.type_ != condition_types::UNKNOWN_CONFIG_FIELDS));
    }

    fn ip_list_config_data(namespace: &str, name: &str, cidr: &str) -> EdgionConfigData {
        use edgion_resources::resources::edgion_config_data::{IpGroup, IpListData};
        build_edgion_config_data(
            namespace,
            name,
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "list".to_string(),
                        description: None,
                        cidrs: vec![cidr.to_string()],
                    }],
                }),
            },
        )
    }

    fn condition_ip_refs(names: &[&str]) -> edgion_resources::resources::edgion_plugins::plugin_configs::Condition {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, IpAccessMatch};
        Condition::DirectPeerIpMatch(IpAccessMatch {
            ip_refs: names
                .iter()
                .map(|name| EdgionConfigDataRef {
                    name: (*name).to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    fn rr_deny_groups(
        groups: Vec<Vec<edgion_resources::resources::edgion_plugins::plugin_configs::Condition>>,
    ) -> edgion_resources::resources::edgion_plugins::plugin_configs::RequestRestrictionConfig {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllOfGroup, AnyOfClause, ConditionSet, RequestRestrictionConfig, RestrictionMode,
        };
        RequestRestrictionConfig {
            mode: Some(RestrictionMode::Deny),
            conditions: ConditionSet::AnyOf {
                any_of: groups
                    .into_iter()
                    .map(|all_of| AnyOfClause::AllOf(AllOfGroup { name: None, all_of }))
                    .collect(),
            },
            ..Default::default()
        }
    }

    fn plugins_with_request_entries(name: &str, entries: Vec<RequestFilterEntry>) -> EdgionPlugins {
        let mut ep = EdgionPlugins::new(name, EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(entries);
        ep
    }

    fn first_rr_condition(
        ep: &EdgionPlugins,
    ) -> &edgion_resources::resources::edgion_plugins::plugin_configs::Condition {
        match &ep.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::RequestRestriction(config) => config
                .conditions
                .iter_conditions()
                .next()
                .expect("request restriction condition"),
            _ => panic!("expected RequestRestriction"),
        }
    }

    fn first_entry_run_condition(
        ep: &EdgionPlugins,
    ) -> &edgion_resources::resources::edgion_plugins::plugin_configs::Condition {
        ep.spec
            .request_plugins
            .as_ref()
            .unwrap()
            .iter()
            .find_map(|entry| {
                entry
                    .conditions
                    .as_ref()
                    .and_then(|conditions| conditions.run.as_ref())
                    .and_then(|conditions| conditions.iter_conditions().next())
            })
            .expect("entry-level run")
    }

    #[tokio::test]
    async fn condition_tree_ip_refs_from_rr_group_and_entry_level_run() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllOfGroup, AnyOfClause, Condition, ConditionSet,
        };
        use edgion_resources::resources::http_route::HTTPHeaderFilter;

        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime.edgion_config_data_store.replace_all(HashMap::from([
            (
                "default/office-nets".to_string(),
                ip_list_config_data("default", "office-nets", "203.0.113.0/24"),
            ),
            (
                "default/backup-nets".to_string(),
                ip_list_config_data("default", "backup-nets", "198.51.100.0/24"),
            ),
        ]));

        let mut run_entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
            set: None,
            add: None,
            remove: None,
        }));
        run_entry.conditions = Some(
            edgion_resources::resources::edgion_plugins::conditions::PluginConditions {
                run: Some(ConditionSet::AnyOf {
                    any_of: vec![
                        AnyOfClause::Condition(condition_ip_refs(&["office-nets"])),
                        AnyOfClause::AllOf(AllOfGroup {
                            name: Some("backup".to_string()),
                            all_of: vec![condition_ip_refs(&["backup-nets"])],
                        }),
                    ],
                }),
                ..Default::default()
            },
        );
        let rr_entry = RequestFilterEntry::new(EdgionPlugin::RequestRestriction(rr_deny_groups(vec![vec![
            condition_ip_refs(&["office-nets"]),
        ]])));
        let parsed = match EdgionPluginsHandler
            .parse(
                plugins_with_request_entries("rr-and-run", vec![rr_entry, run_entry]),
                &ctx,
            )
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        let Condition::DirectPeerIpMatch(rr_ip) = first_rr_condition(&parsed) else {
            panic!("expected RR ipRefs condition")
        };
        assert_eq!(
            rr_ip.resolved_ips.as_deref(),
            Some(["203.0.113.0/24".to_string()].as_slice())
        );
        let Condition::DirectPeerIpMatch(run_ip) = first_entry_run_condition(&parsed) else {
            panic!("expected entry-level run ipRefs")
        };
        assert_eq!(
            run_ip.resolved_ips.as_deref(),
            Some(["203.0.113.0/24".to_string()].as_slice())
        );
        let run = parsed.spec.request_plugins.as_ref().unwrap()[1]
            .conditions
            .as_ref()
            .and_then(|conditions| conditions.run.as_ref())
            .expect("entry-level run");
        let resolved: Vec<_> = run
            .iter_conditions()
            .map(|condition| match condition {
                Condition::DirectPeerIpMatch(ip) => ip.resolved_ips.clone().unwrap(),
                _ => panic!("expected entry-level run ipRefs"),
            })
            .collect();
        assert_eq!(
            resolved,
            vec![vec!["203.0.113.0/24".to_string()], vec!["198.51.100.0/24".to_string()],]
        );
        let refs = runtime.edgion_config_data_refs.get_refs("default/office-nets");
        assert!(
            refs.contains(&"EdgionPlugins:default/rr-and-run".to_string()),
            "RR group and entry-level run must register the same owner for requeue, got: {refs:?}"
        );
        assert!(runtime
            .edgion_config_data_refs
            .get_refs("default/backup-nets")
            .contains(&"EdgionPlugins:default/rr-and-run".to_string()));
        assert!(super::resolve_ip_list_refs(&parsed, &ctx).is_empty());
    }

    #[tokio::test]
    async fn condition_tree_missing_ip_ref_sets_resolved_refs_false() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::Condition;

        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::new());
        let parsed = match EdgionPluginsHandler
            .parse(
                plugins_with_request_entries(
                    "missing-ipref",
                    vec![RequestFilterEntry::new(EdgionPlugin::RequestRestriction(
                        rr_deny_groups(vec![vec![condition_ip_refs(&["missing-nets"])]]),
                    ))],
                ),
                &ctx,
            )
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let Condition::DirectPeerIpMatch(ip) = first_rr_condition(&parsed) else {
            panic!("expected RR ipRefs condition")
        };
        assert!(
            ip.resolved_ips.is_none(),
            "missing operator ipRefs must not compile an empty resolvedIps matcher"
        );

        let mut status_ep = parsed;
        EdgionPluginsHandler.update_status(&mut status_ep, &ctx, &[]);
        let resolved = status_ep
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .find(|condition| condition.type_ == "ResolvedRefs")
            .expect("ResolvedRefs condition must be present");
        assert_eq!(resolved.status, "False");
    }

    #[tokio::test]
    async fn condition_tree_ignores_refs_on_disabled_entries() {
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        let entry = RequestFilterEntry::with_enable(
            EdgionPlugin::RequestRestriction(rr_deny_groups(vec![vec![condition_ip_refs(&["inactive-nets"])]])),
            false,
        );
        let parsed = match EdgionPluginsHandler
            .parse(plugins_with_request_entries("disabled-ref", vec![entry]), &ctx)
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        assert!(runtime
            .edgion_config_data_refs
            .get_refs("default/inactive-nets")
            .is_empty());
        assert!(super::resolve_ip_list_refs(&parsed, &ctx).is_empty());
    }

    #[tokio::test]
    async fn condition_tree_config_data_change_requeues_entry_level_and_rr() {
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime.edgion_config_data_store.replace_all(HashMap::from([(
            "default/watch-nets".to_string(),
            ip_list_config_data("default", "watch-nets", "198.51.100.0/24"),
        )]));

        let mut run_entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
            edgion_resources::resources::http_route::HTTPHeaderFilter {
                set: None,
                add: None,
                remove: None,
            },
        ));
        run_entry.conditions = Some(
            edgion_resources::resources::edgion_plugins::conditions::PluginConditions {
                run: Some(
                    edgion_resources::resources::edgion_plugins::conditions::ConditionSet::all_of(vec![
                        condition_ip_refs(&["watch-nets"]),
                    ]),
                ),
                ..Default::default()
            },
        );
        let parsed = match EdgionPluginsHandler
            .parse(
                plugins_with_request_entries(
                    "watch-iprefs",
                    vec![
                        RequestFilterEntry::new(EdgionPlugin::RequestRestriction(rr_deny_groups(vec![vec![
                            condition_ip_refs(&["watch-nets"]),
                        ]]))),
                        run_entry,
                    ],
                ),
                &ctx,
            )
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let refs = runtime.edgion_config_data_refs.get_refs("default/watch-nets");
        assert!(
            refs.contains(&"EdgionPlugins:default/watch-iprefs".to_string()),
            "ConfigData change must requeue the owning EdgionPlugins, got: {refs:?}"
        );
        assert!(super::resolve_ip_list_refs(&parsed, &ctx).is_empty());
    }

    #[tokio::test]
    async fn condition_tree_expansion_then_credential_resolves_imported_hmac() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, HmacMatchConfig};

        let condition = Condition::HmacAuth(HmacMatchConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "vendor".to_string(),
                description: None,
                secret_refs: vec![SecretObjectReference {
                    name: "vendor-hmac".to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                }],
            }]),
            ..Default::default()
        });
        let credentials = "- username: vendor\n  secret: vendor-secret-at-least-32-bytes-0001\n";
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::from([(
            "default/vendor-hmac".to_string(),
            build_secret("default", "vendor-hmac", "credentials.yaml", credentials),
        )]));
        runtime.edgion_config_data_store.replace_all(HashMap::from([(
            "default/vendor-list".to_string(),
            request_access_url_data(
                "default",
                "vendor-list",
                vec![request_access_url_candidate("vendor", vec![condition])],
            ),
        )]));

        let resource_ref = ResourceRef::new(
            edgion_resources::ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "url-policy".to_string(),
        );
        let mut credential_first = request_access_url_policy("                - name: vendor-list");
        super::super::plugins::condition_tree::resolve_credentials(&mut credential_first, &resource_ref, &ctx);
        let EdgionPlugin::RequestAccessPolicy(config) =
            &credential_first.spec.request_plugins.as_ref().unwrap()[0].plugin
        else {
            panic!("expected RequestAccessPolicy")
        };
        assert!(
            config.resolved_profiles.is_none(),
            "credential pass before expansion must not see imported HMAC secretGroups"
        );

        let mut expanded_then_credentials = request_access_url_policy("                - name: vendor-list");
        super::super::plugins::condition_tree::expand_config_data(&mut expanded_then_credentials, &resource_ref, &ctx);
        super::super::plugins::condition_tree::resolve_credentials(&mut expanded_then_credentials, &resource_ref, &ctx);
        let Condition::HmacAuth(hmac) =
            first_url_candidate_condition(&first_resolved_url_candidates(&expanded_then_credentials)[0])
        else {
            panic!("expected nested HMAC after expansion")
        };
        assert!(
            hmac.resolved_credentials
                .as_ref()
                .is_some_and(|credentials| credentials.contains_key("vendor")),
            "expansion then credentials must resolve HMAC secretGroups imported from ConfigData"
        );
    }

    #[tokio::test]
    async fn condition_tree_expansion_has_no_resource_ip_match_cap() {
        use edgion_resources::resources::edgion_plugins::conditions::PluginConditions;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, ConditionSet, IpAccessMatch};

        let imported = request_access_url_candidate(
            "vendor",
            vec![
                Condition::DirectPeerIpMatch(IpAccessMatch {
                    ips: vec!["192.0.2.0/24".to_string()],
                    ..Default::default()
                }),
                Condition::RemoteIpMatch(IpAccessMatch {
                    ips: vec!["198.51.100.0/24".to_string()],
                    ..Default::default()
                }),
            ],
        );
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::new());
        runtime.edgion_config_data_store.replace_all(HashMap::from([(
            "default/vendor-list".to_string(),
            request_access_url_data("default", "vendor-list", vec![imported]),
        )]));

        let mut authored = request_access_url_policy("                - name: vendor-list");
        // Two imported matches take the resource beyond the former 10,000-node cap.
        authored.spec.request_plugins.as_mut().unwrap()[0].conditions = Some(PluginConditions {
            run: Some(ConditionSet::all_of(vec![
                Condition::DirectPeerIpMatch(IpAccessMatch {
                    ips: vec!["192.0.2.0/24".to_string()],
                    ..Default::default()
                });
                10_000
            ])),
            ..Default::default()
        });
        let parsed = match EdgionPluginsHandler.parse(authored, &ctx).await {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let errors = parsed.validate_plugins();
        assert!(errors.is_empty(), "expanded resource must validate: {errors:?}");
        let EdgionPlugin::RequestAccessPolicy(config) = &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected RequestAccessPolicy");
        };
        assert!(
            config.resolution_warnings.is_empty(),
            "{:?}",
            config.resolution_warnings
        );
        assert_eq!(first_resolved_url_candidates(&parsed)[0].ip_rule_count(), 2);
    }

    #[tokio::test]
    async fn condition_tree_rr_hmac_resolves_credentials() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{Condition, HmacMatchConfig};

        let credentials = "- username: rr-alice\n  secret: rr-hmac-secret-at-least-32-bytes-0001\n";
        let (ctx, _runtime) = make_ctx_with_secrets(HashMap::from([(
            "default/rr-hmac".to_string(),
            build_secret("default", "rr-hmac", "credentials.yaml", credentials),
        )]));
        let parsed = match EdgionPluginsHandler
            .parse(
                plugins_with_request_entries(
                    "rr-hmac",
                    vec![RequestFilterEntry::new(EdgionPlugin::RequestRestriction(
                        rr_deny_groups(vec![vec![Condition::HmacAuth(HmacMatchConfig {
                            secret_groups: Some(vec![HmacAuthSecretGroup {
                                name: "rr".to_string(),
                                description: None,
                                secret_refs: vec![SecretObjectReference {
                                    name: "rr-hmac".to_string(),
                                    group: None,
                                    kind: None,
                                    namespace: None,
                                }],
                            }]),
                            ..Default::default()
                        })]]),
                    ))],
                ),
                &ctx,
            )
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        let Condition::HmacAuth(hmac) = first_rr_condition(&parsed) else {
            panic!("expected RR HMAC condition")
        };
        assert!(
            hmac.resolved_credentials
                .as_ref()
                .is_some_and(|credentials| credentials.contains_key("rr-alice")),
            "RR HMAC must reuse hmac.rs credential resolution"
        );
        assert_eq!(ctx.secret_ref_manager().get_refs("default/rr-hmac").len(), 1);
    }

    #[tokio::test]
    async fn condition_tree_body_and_dye_walk_both_passes_and_clear_removed_refs() {
        use edgion_resources::common::KeyGet;
        use edgion_resources::resources::edgion_plugins::conditions::PluginConditions;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllOfGroup, AnyOfClause, Condition, ConditionSet, HmacMatchConfig, RequestBodyBufferConfig,
            SecretMatchCondition,
        };
        use edgion_resources::resources::edgion_plugins::{BodyRequirement, DyeOutcome, DyeRule, DyeSuite};

        fn secret_match(name: &str) -> Condition {
            Condition::SecretMatch(SecretMatchCondition {
                key: KeyGet::Header {
                    name: "X-Condition-Token".to_string(),
                },
                secret_key: "token".to_string(),
                secret_ref: SecretObjectReference {
                    name: name.to_string(),
                    group: None,
                    kind: None,
                    namespace: None,
                },
                negate: false,
                resolved_values: None,
                descriptor: Box::from(""),
            })
        }

        fn hmac_match(name: &str, group: &str) -> Condition {
            Condition::HmacAuth(HmacMatchConfig {
                secret_groups: Some(vec![HmacAuthSecretGroup {
                    name: group.to_string(),
                    description: None,
                    secret_refs: vec![SecretObjectReference {
                        name: name.to_string(),
                        group: None,
                        kind: None,
                        namespace: None,
                    }],
                }]),
                ..Default::default()
            })
        }

        fn mixed_conditions(direct: Condition, nested: Vec<Condition>) -> PluginConditions {
            PluginConditions {
                run: Some(ConditionSet::AnyOf {
                    any_of: vec![
                        AnyOfClause::Condition(direct),
                        AnyOfClause::AllOf(AllOfGroup {
                            name: Some("nested".to_string()),
                            all_of: nested,
                        }),
                    ],
                }),
                ..Default::default()
            }
        }

        let credentials = "- username: condition-user\n  secret: condition-secret-at-least-32-bytes-0001\n";
        let (ctx, runtime) = make_ctx_with_secrets(HashMap::from([
            (
                "default/body-token".to_string(),
                build_secret("default", "body-token", "token", "body-value"),
            ),
            (
                "default/dye-token".to_string(),
                build_secret("default", "dye-token", "token", "dye-value"),
            ),
            (
                "default/condition-hmac".to_string(),
                build_secret("default", "condition-hmac", "credentials.yaml", credentials),
            ),
        ]));
        runtime.edgion_config_data_store.replace_all(HashMap::from([
            (
                "default/body-nets".to_string(),
                ip_list_config_data("default", "body-nets", "192.0.2.0/24"),
            ),
            (
                "default/dye-nets".to_string(),
                ip_list_config_data("default", "dye-nets", "198.51.100.0/24"),
            ),
        ]));

        let mut body_entry =
            RequestFilterEntry::new(EdgionPlugin::RequestBodyBuffer(RequestBodyBufferConfig::default()));
        body_entry.body = Some(BodyRequirement {
            conditions: Some(mixed_conditions(
                condition_ip_refs(&["body-nets"]),
                vec![secret_match("body-token"), hmac_match("condition-hmac", "body")],
            )),
            ..Default::default()
        });

        let mut dye_entry = RequestFilterEntry::new(EdgionPlugin::RegionRoute(Default::default()));
        dye_entry.dye = Some(DyeSuite {
            request: Vec::new(),
            response: vec![DyeRule {
                name: "X-Condition-Dye".to_string(),
                on: vec![DyeOutcome::Success],
                value: None,
                conditions: Some(mixed_conditions(
                    secret_match("dye-token"),
                    vec![condition_ip_refs(&["dye-nets"]), hmac_match("condition-hmac", "dye")],
                )),
            }],
        });

        let parsed = match EdgionPluginsHandler
            .parse(
                plugins_with_request_entries("body-dye-refs", vec![body_entry, dye_entry]),
                &ctx,
            )
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };

        let body_conditions = parsed.spec.request_plugins.as_ref().unwrap()[0]
            .body
            .as_ref()
            .and_then(|body| body.conditions.as_ref())
            .and_then(|conditions| conditions.run.as_ref())
            .expect("body conditions");
        let body_atoms: Vec<_> = body_conditions.iter_conditions().collect();
        let Condition::DirectPeerIpMatch(body_ip) = body_atoms[0] else {
            panic!("expected direct body IP condition")
        };
        assert_eq!(
            body_ip.resolved_ips.as_deref(),
            Some(["192.0.2.0/24".to_string()].as_slice())
        );
        let Condition::SecretMatch(body_secret) = body_atoms[1] else {
            panic!("expected nested body SecretMatch condition")
        };
        assert_eq!(
            body_secret.resolved_values.as_deref(),
            Some(["body-value".to_string()].as_slice())
        );
        let Condition::HmacAuth(body_hmac) = body_atoms[2] else {
            panic!("expected nested body HMAC condition")
        };
        assert!(body_hmac
            .resolved_credentials
            .as_ref()
            .is_some_and(|credentials| credentials.contains_key("condition-user")));

        let dye_conditions = parsed.spec.request_plugins.as_ref().unwrap()[1]
            .dye
            .as_ref()
            .and_then(|dye| dye.response[0].conditions.as_ref())
            .and_then(|conditions| conditions.run.as_ref())
            .expect("dye conditions");
        let dye_atoms: Vec<_> = dye_conditions.iter_conditions().collect();
        let Condition::SecretMatch(dye_secret) = dye_atoms[0] else {
            panic!("expected direct dye SecretMatch condition")
        };
        assert_eq!(
            dye_secret.resolved_values.as_deref(),
            Some(["dye-value".to_string()].as_slice())
        );
        let Condition::DirectPeerIpMatch(dye_ip) = dye_atoms[1] else {
            panic!("expected nested dye IP condition")
        };
        assert_eq!(
            dye_ip.resolved_ips.as_deref(),
            Some(["198.51.100.0/24".to_string()].as_slice())
        );
        let Condition::HmacAuth(dye_hmac) = dye_atoms[2] else {
            panic!("expected nested dye HMAC condition")
        };
        assert!(dye_hmac
            .resolved_credentials
            .as_ref()
            .is_some_and(|credentials| credentials.contains_key("condition-user")));

        for key in ["default/body-nets", "default/dye-nets"] {
            assert_eq!(
                runtime.edgion_config_data_refs.get_refs(key),
                vec!["EdgionPlugins:default/body-dye-refs".to_string()],
                "condition IP ref must register its owner: {key}"
            );
        }
        for key in ["default/body-token", "default/dye-token", "default/condition-hmac"] {
            assert_eq!(
                ctx.secret_ref_manager().get_refs(key),
                vec![ResourceRef::new(
                    edgion_resources::ResourceKind::EdgionPlugins,
                    Some("default".to_string()),
                    "body-dye-refs".to_string(),
                )],
                "condition Secret ref must register its owner: {key}"
            );
        }

        let reparsed = match EdgionPluginsHandler
            .parse(
                plugins_with_request_entries(
                    "body-dye-refs",
                    vec![
                        RequestFilterEntry::new(EdgionPlugin::RequestBodyBuffer(RequestBodyBufferConfig::default())),
                        RequestFilterEntry::new(EdgionPlugin::RegionRoute(Default::default())),
                    ],
                ),
                &ctx,
            )
            .await
        {
            ProcessResult::Continue(value) => value,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(reparsed.spec.request_plugins.as_ref().unwrap()[0].body.is_none());
        assert!(reparsed.spec.request_plugins.as_ref().unwrap()[1].dye.is_none());
        for key in ["default/body-nets", "default/dye-nets"] {
            assert!(
                runtime.edgion_config_data_refs.get_refs(key).is_empty(),
                "removed condition IP ref must clear reverse refs: {key}"
            );
        }
        for key in ["default/body-token", "default/dye-token", "default/condition-hmac"] {
            assert!(
                ctx.secret_ref_manager().get_refs(key).is_empty(),
                "removed condition Secret ref must clear reverse refs: {key}"
            );
        }
    }
}
