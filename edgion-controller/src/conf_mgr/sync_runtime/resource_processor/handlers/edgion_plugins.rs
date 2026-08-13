//! EdgionPlugins Handler
//!
//! Handles EdgionPlugins resources with Gateway API standard status management.
//! Delegates per-plugin Secret resolution to `plugins::resolve_credential`.
//!
//! Features:
//! - parse: Resolve Secret references and register to SecretRefManager for cascading updates
//! - parse: Register `overrideRef` EdgionConfigData → EP in `edgion_config_data_refs` for requeue on change
//! - on_delete: Clear SecretRefManager references + EdgionConfigData refs
//! - update_status: Validate `overrideRef` resolves to a `RegionRouteOverride` EdgionConfigData

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, partially_invalid_condition, pick_resolved_refs_reason,
    update_condition, ResolvedRefsError,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_types, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
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
    /// Clear all EdgionConfigData reverse refs (overrideRef + IpList allow/deny refs) registered by this EdgionPlugins.
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
    /// plugin's `allow_refs`/`deny_refs` in `request_plugins`. Namespace defaults to the
    /// EP namespace.
    fn get_ip_list_ref_keys(ep: &EdgionPlugins) -> Vec<String> {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let Some(plugins) = &ep.spec.request_plugins else {
            return vec![];
        };
        let mut keys = Vec::new();
        for e in plugins {
            if let EdgionPlugin::IpRestriction(ref config) = e.plugin {
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
        keys
    }

    /// Collect "namespace/name" keys for every `GlobalAccessControl` plugin's
    /// `active_profile_ref` in `request_plugins`. Namespace defaults to the EP namespace.
    /// Mirrors `get_region_route_override_keys` for the GAC ref site.
    fn get_active_profile_ref_keys(ep: &EdgionPlugins) -> Vec<String> {
        let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
        let Some(plugins) = &ep.spec.request_plugins else {
            return vec![];
        };
        plugins
            .iter()
            .filter_map(|e| {
                if let EdgionPlugin::GlobalAccessControl(ref config) = e.plugin {
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
                EdgionPlugin::GlobalAccessControl(config) => {
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
                | EdgionPlugin::A2aGateway(_)
                | EdgionPlugin::ResponseHeaderModifier(_)
                | EdgionPlugin::RequestRedirect(_)
                | EdgionPlugin::UrlRewrite(_)
                | EdgionPlugin::RequestMirror(_)
                | EdgionPlugin::BasicAuth(_)
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

        cred::resolve_basic_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_jwt(&mut ep, &resource_ref, ctx);
        cred::resolve_jwe(&mut ep, &resource_ref, ctx);
        cred::resolve_header_cert_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_hmac(&mut ep, &resource_ref, ctx);
        cred::resolve_key_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_openid_connect(&mut ep, &resource_ref, ctx);
        cred::resolve_ext_proc(&mut ep, &resource_ref, ctx);
        cred::resolve_ldap_auth(&mut ep, &resource_ref, ctx);
        cred::resolve_csrf(&mut ep, &resource_ref, ctx);
        cred::resolve_global_access_control(&mut ep, &resource_ref, ctx);
        cred::resolve_forward_auth(&mut ep, &resource_ref, ctx).await;
        cred::resolve_guardrail(&mut ep, &resource_ref, ctx).await;
        cred::resolve_ai_guard(&mut ep, &resource_ref, ctx).await;
        cred::resolve_wasm(&mut ep, &resource_ref, ctx);

        // Clear existing EdgionConfigData override refs for this EP, then re-register from scratch.
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

        // Register requeue watcher: EP → EdgionConfigData(GlobalAccessControl activeProfileRef).
        // Closes a pre-existing gap: active_profile_ref was not tracked before Task 6.
        for gac_key in Self::get_active_profile_ref_keys(&ep) {
            ctx.runtime().edgion_config_data_refs.add_ref(gac_key, ep_key.clone());
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
        cross_mgr.clear_resource_refs(&resource_ref);
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
        Self::clear_config_data_refs(ep, ctx);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);
        requeue_referencing_resources(ep, ctx).await;

        tracing::debug!(
            edgion_plugins = %resource_ref.key(),
            "Cleared secret, EdgionConfigData, and cross-ns references on EdgionPlugins delete"
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

        let mut resolved_errors = Self::resolve_secret_refs(ep, ctx);
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
        let partially_invalid = if inline_errors.is_empty() {
            partially_invalid_condition(None, generation)
        } else {
            let msg = edgion_resources::matcher::ip_cidr_helpers::format_entry_errors(&inline_errors);
            partially_invalid_condition(Some(msg), generation)
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
    }
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
/// or one `EdgionConfigDataNotFound` error per missing or wrong-type reference.
/// Mutually exclusive per ref via else-branch to avoid a TOCTOU double-message.
pub fn resolve_override_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
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
                    let resolved = target
                        .as_ref()
                        .is_some_and(|t| matches!(&t.spec.data, ConfigEntry::RegionRouteOverride(_)));
                    if !resolved {
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
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
        if let EdgionPlugin::IpRestriction(ref config) = entry.plugin {
            for r in config
                .allow_refs
                .iter()
                .flatten()
                .chain(config.deny_refs.iter().flatten())
            {
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
            }
        }
    }
    errors
}

/// Check each `GlobalAccessControl` plugin's `active_profile_ref` against the EdgionConfigData
/// store. Returns one `RefNotPermitted` error when `ref_denied` is set (cross-namespace denial),
/// or one `EdgionConfigDataNotFound { expected_type: "Selector" }` per missing or wrong-type
/// reference. Mutually exclusive per ref via else-branch to avoid a TOCTOU double-message.
///
/// This is a net-new resolver — `GlobalAccessControl.active_profile_ref` (the 7th ref site)
/// had no status reporting before Task 5.
pub fn resolve_active_profile_refs(ep: &EdgionPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let ns = ep.metadata.namespace.as_deref().unwrap_or("default");
    let Some(plugins) = &ep.spec.request_plugins else {
        return vec![];
    };
    let mut errors = Vec::new();
    for entry in plugins {
        if let EdgionPlugin::GlobalAccessControl(ref config) = entry.plugin {
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
/// or wrong-type reference. Mirrors `resolve_active_profile_refs` (GlobalAccessControl).
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

/// Requeue all Gateways and HTTPRoutes that reference this EdgionPlugins.
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

#[cfg(test)]
mod tests {
    use super::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler};
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::prelude_resources::EdgionConfigData;
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigDataRef, EdgionConfigDataSpec, RegionRouteOverride,
    };
    use edgion_resources::resources::edgion_plugins::{
        BasicAuthConfig, BasicAuthSecretGroup, CertSourceMode, EdgionPlugin, EdgionPlugins, EdgionPluginsSpec,
        HeaderCertAuthConfig, HmacAuthConfig, HmacAuthSecretGroup, JweDecryptConfig, OpenidConnectConfig,
        RegionRouteConfig, RequestFilterEntry,
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

        let (ctx, _runtime) = make_ctx_with_secrets(secrets);

        let mut ep = EdgionPlugins::new("hmac-plugins", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::HmacAuth(HmacAuthConfig {
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
            upstream_header_fields: vec!["X-Consumer-Username".to_string()],
            ..Default::default()
        }))]);

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

    // ── Canary + activeProfileRef resolution tests ──────────────────────────

    #[tokio::test]
    async fn test_resolve_canary_active_profile_refs_ok() {
        // Selector EdgionConfigData exists → resolve_canary_active_profile_refs returns no error.
        use edgion_resources::resources::edgion_config_data::Selector;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget};

        let selector_ed = build_edgion_config_data(
            "default",
            "canary-selector",
            EdgionConfigDataSpec {
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
            on_invalid: Default::default(),
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
        use edgion_resources::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget};

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
            on_invalid: Default::default(),
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
        use edgion_resources::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget};

        let wrong_type_ed = build_edgion_config_data(
            "default",
            "canary-selector-wrong-type",
            EdgionConfigDataSpec {
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
            on_invalid: Default::default(),
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
        // Verify the Canary active_profile_ref cross-ns gap is closed the same way as GAC's
        // (Task 6, Step 3): both the same-resource requeue loop and the cross_mgr loop in
        // parse() must register the ref, otherwise a ReferenceGrant change in the target
        // namespace would silently fail to requeue this EdgionPlugins.
        use edgion_resources::resources::edgion_config_data::Selector;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget};
        use edgion_resources::ResourceKind;

        let selector_ed = build_edgion_config_data(
            "shared-ns",
            "canary-selector",
            EdgionConfigDataSpec {
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
            on_invalid: Default::default(),
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

    // ===== GlobalAccessControl SecretRef resolution (restored; see Plan 3) =====

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
    async fn cross_ns_gac_ref_registered_for_grant_revalidation() {
        // Verify the GAC active_profile_ref cross-ns gap is also closed (Task 6 Step 3a).
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, Selector};
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllowProfile, AllowRuleGroup, GlobalAccessControlConfig,
        };
        use edgion_resources::resources::edgion_plugins::{DefaultAction, IpSource};
        use edgion_resources::ResourceKind;

        let selector_ed = build_edgion_config_data(
            "shared-ns",
            "gac-selector",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
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

        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: None,
                        ip: None,
                        rules: vec![],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: Some(EdgionConfigDataRef {
                name: "gac-selector".into(),
                namespace: Some("shared-ns".into()),
                ..Default::default()
            }),
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
        };

        let mut ep = EdgionPlugins::new("gac-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::GlobalAccessControl(cfg))]);

        let handler = EdgionPluginsHandler::new();
        let _ = handler.parse(ep, &ctx).await;

        // cross_ns_ref_mgr must contain this EP under "shared-ns".
        let refs = runtime.cross_ns_ref_mgr.get_refs("shared-ns");
        assert!(
            refs.iter().any(|rr| rr.kind == ResourceKind::EdgionPlugins),
            "GAC active_profile_ref cross-ns must register EP in cross_ns_ref_mgr; got: {:?}",
            refs
        );

        // edgion_config_data_refs must also track GAC (the pre-existing gap is now closed).
        let ep_key = "EdgionPlugins:team-a/gac-ep";
        let cd_refs = runtime.edgion_config_data_refs.get_refs("shared-ns/gac-selector");
        assert!(
            cd_refs.contains(&ep_key.to_string()),
            "GAC active_profile_ref must be registered in edgion_config_data_refs; got: {:?}",
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
        // Only RegionRoute, IpRestriction, GlobalAccessControl, and Canary carry
        // EdgionConfigData refs; everything else must map to false.
        fn carries_config_data_ref(p: &EdgionPlugin) -> bool {
            match p {
                EdgionPlugin::RegionRoute(_)
                | EdgionPlugin::IpRestriction(_)
                | EdgionPlugin::GlobalAccessControl(_)
                | EdgionPlugin::Canary(_) => true,
                EdgionPlugin::RequestHeaderModifier(_)
                | EdgionPlugin::AiProxy(_)
                | EdgionPlugin::A2aGateway(_)
                | EdgionPlugin::ResponseHeaderModifier(_)
                | EdgionPlugin::RequestRedirect(_)
                | EdgionPlugin::UrlRewrite(_)
                | EdgionPlugin::RequestMirror(_)
                | EdgionPlugin::BasicAuth(_)
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
                | EdgionPlugin::Wasm(_) => false,
            }
        }
        let _ = carries_config_data_ref; // referenced for exhaustiveness guard
    }

    #[tokio::test]
    async fn global_access_control_resolves_secret_values() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllowProfile, AllowRuleGroup, GlobalAccessControlConfig, RestrictionRule, RestrictionSource,
        };
        use edgion_resources::resources::edgion_plugins::{DefaultAction, IpSource};

        let mut secrets = HashMap::new();
        secrets.insert(
            "default/cf-origin-secret".to_string(),
            build_secret("default", "cf-origin-secret", "token", "s3cr3t-TOKEN"),
        );
        let (ctx, _rt) = make_ctx_with_secrets(secrets);

        let rule = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-CF-Secret".into()),
            secret_ref: Some(SecretObjectReference {
                name: "cf-origin-secret".into(),
                group: None,
                kind: None,
                namespace: None,
            }),
            ..Default::default()
        };
        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: Some("cloudflare".into()),
                        ip: None,
                        rules: vec![rule],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
        };

        let mut ep = EdgionPlugins::new("origin-lockdown", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::GlobalAccessControl(cfg))]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::GlobalAccessControl(c) => c,
            _ => panic!("unexpected plugin type"),
        };
        let resolved = config.profiles["lockdown"].rule_groups[0].rules[0]
            .resolved_secret_values
            .as_ref()
            .expect("expected resolved secret values");
        assert!(resolved.contains(&"s3cr3t-TOKEN".to_string()));
        assert_eq!(ctx.secret_ref_manager().get_refs("default/cf-origin-secret").len(), 1);
    }

    #[tokio::test]
    async fn global_access_control_no_secret_leaves_values_none() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllowProfile, AllowRuleGroup, GlobalAccessControlConfig, RestrictionRule, RestrictionSource,
        };
        use edgion_resources::resources::edgion_plugins::{DefaultAction, IpSource};

        let (ctx, _rt) = make_ctx_with_secrets(HashMap::new());

        let rule = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-CF-Secret".into()),
            values: Some(vec!["inline".into()]),
            ..Default::default()
        };
        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: Some("g".into()),
                        ip: None,
                        rules: vec![rule],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
        };

        let mut ep = EdgionPlugins::new("p", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::GlobalAccessControl(cfg))]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::GlobalAccessControl(c) => c,
            _ => panic!("unexpected plugin type"),
        };
        assert!(config.profiles["lockdown"].rule_groups[0].rules[0]
            .resolved_secret_values
            .is_none());
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
    async fn gac_denied_active_profile_ref_reports_ref_not_permitted() {
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, Selector};
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllowProfile, AllowRuleGroup, GlobalAccessControlConfig,
        };
        use edgion_resources::resources::edgion_plugins::{DefaultAction, IpSource};

        // Seed a Selector EdgionConfigData in team-b (cross-namespace target, right type).
        let selector_ed = build_edgion_config_data(
            "team-b",
            "selector-cfg",
            EdgionConfigDataSpec {
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

        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: None,
                        ip: None,
                        rules: vec![],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: Some(EdgionConfigDataRef {
                name: "selector-cfg".into(),
                namespace: Some("team-b".into()),
                ..Default::default()
            }),
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
        };

        let mut ep = EdgionPlugins::new("gac-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("team-a".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::GlobalAccessControl(cfg))]);

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
            "denied cross-ns GAC active_profile_ref must yield reason RefNotPermitted"
        );
    }

    #[tokio::test]
    async fn disabled_selector_active_profile_ref_reports_not_found() {
        use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, Selector};
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllowProfile, AllowRuleGroup, GlobalAccessControlConfig,
        };
        use edgion_resources::resources::edgion_plugins::{DefaultAction, IpSource};

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

        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: None,
                        ip: None,
                        rules: vec![],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: Some(EdgionConfigDataRef {
                name: "disabled-selector".into(),
                namespace: None,
                ..Default::default()
            }),
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
        };

        let mut ep = EdgionPlugins::new("gac-disabled-ep", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::GlobalAccessControl(cfg))]);

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
    async fn global_access_control_missing_secret_clears_stale_values() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::{
            AllowProfile, AllowRuleGroup, GlobalAccessControlConfig, RestrictionRule, RestrictionSource,
        };
        use edgion_resources::resources::edgion_plugins::{DefaultAction, IpSource};

        let (ctx, _rt) = make_ctx_with_secrets(HashMap::new());

        let mut rule = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-CF-Secret".into()),
            secret_ref: Some(SecretObjectReference {
                name: "cf-origin-secret".into(),
                group: None,
                kind: None,
                namespace: None,
            }),
            ..Default::default()
        };
        rule.resolved_secret_values = Some(vec!["OLD-TOKEN".into()]);

        let cfg = GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([(
                "lockdown".to_string(),
                AllowProfile {
                    default_action: DefaultAction::Deny,
                    rule_groups: vec![AllowRuleGroup {
                        name: Some("cloudflare".into()),
                        ip: None,
                        rules: vec![rule],
                        ip_matcher: None,
                    }],
                },
            )]),
            description: None,
            active_profile_ref: None,
            status: 403,
            message: None,
            ip_source: IpSource::RemoteAddr,
        };

        let mut ep = EdgionPlugins::new("p", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::GlobalAccessControl(cfg))]);

        let parsed = match EdgionPluginsHandler.parse(ep, &ctx).await {
            ProcessResult::Continue(v) => v,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {}", reason),
        };
        let config = match &parsed.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::GlobalAccessControl(c) => c,
            _ => panic!("unexpected plugin type"),
        };
        assert!(config.profiles["lockdown"].rule_groups[0].rules[0]
            .resolved_secret_values
            .is_none());
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
}
