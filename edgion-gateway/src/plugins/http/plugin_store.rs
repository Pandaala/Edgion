//! Plugin source inventory and scoped-view compilation.
//!
//! The canonical publisher owns all executable views and shares stateful entry
//! implementations across GatewayClass scopes. This adapter exposes source counts
//! and submits conf-sync inputs; requests retain their selected Gateway scope.

#[cfg(test)]
use std::collections::HashMap;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use crate::plugins::runtime::conditions::PluginConditionsExt;
use crate::plugins::runtime::entry_bundle::EntryCache;
use crate::plugins::runtime::{InvalidUpdatePolicy, PluginRuntime};
use edgion_resources::resource::meta::ResourceMeta;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::EdgionPlugins;

static GLOBAL_PLUGIN_STORE: LazyLock<Arc<PluginStore>> = LazyLock::new(|| Arc::new(PluginStore { isolated: None }));

pub fn get_global_plugin_store() -> Arc<PluginStore> {
    GLOBAL_PLUGIN_STORE.clone()
}

/// Borrow the source-inventory and conf-sync adapter.
pub fn global_plugin_store() -> &'static PluginStore {
    &GLOBAL_PLUGIN_STORE
}

/// Compiled EdgionPlugins: immutable schema + the gateway plugin runtime.
///
/// This is the gateway-side companion to the `EdgionPlugins` schema. The schema
/// (`types`) carries no runtime; all runtime construction (`PluginRuntime`,
/// compiled conditions) happens here in `compile_plugins`.
pub struct CompiledPlugins {
    /// The serializable EdgionPlugins schema (with conditions compiled in place
    /// for the spec copy — request-time execution uses `plugin_runtime`, not the
    /// spec's conditions; the spec is retained for store stats / lookups).
    pub spec: Arc<EdgionPlugins>,
    /// One immutable effective runtime view. Its entry filters may be shared
    /// with other views of the same source generation.
    pub plugin_runtime: Arc<PluginRuntime>,
    /// Gateway-side preparse/validation diagnostics collected while building
    /// the runtime. The Controller independently mirrors static diagnostics via
    /// `EdgionPlugins::validate_plugins` when writing the resource status.
    pub preparse_errors: Vec<String>,
    /// Whether this compiled resource is safe to activate.
    ///
    /// Some preparse errors only sanitize non-runtime declarations, so
    /// `preparse_errors.is_empty()` is not an activation predicate. Activation
    /// is derived from `PluginRuntime::invalid_plugins` before sealing.
    pub is_activatable: bool,
    /// Whether an invalid update should retain the prior activatable runtime.
    ///
    /// DSL/WAF programs allow last-known-good publication unless either
    /// generation contains a cross-namespace mirror or an enabled JWE entry.
    /// Other plugin configuration errors keep the established fail-closed
    /// replacement behavior. The scoped publisher reapplies the current
    /// execution policy to retained source entries.
    pub retain_previous_on_invalid_update: bool,
}

impl CompiledPlugins {
    /// Reusing a whole runtime also reuses its mirror authorization snapshot.
    /// Cross-namespace mirrors in either generation therefore require publishing
    /// the incoming sealed runtime, even when a DSL/WAF update is invalid.
    pub(crate) fn can_retain_previous(&self, previous: &Self) -> bool {
        self.retain_previous_on_invalid_update
            && previous.is_activatable
            && !self.has_cross_namespace_mirror()
            && !previous.has_cross_namespace_mirror()
            && !self.has_enabled_jwe()
            && !previous.has_enabled_jwe()
            && !self.spec.has_active_grpc_transcoder()
            && !previous.spec.has_active_grpc_transcoder()
            && !self.spec.has_active_graphql_operation_router()
            && !previous.spec.has_active_graphql_operation_router()
    }

    fn has_cross_namespace_mirror(&self) -> bool {
        let namespace = self.spec.metadata.namespace.as_deref().unwrap_or("default");
        self.spec.spec.request_plugins.iter().flatten().any(|entry| {
            if !entry.is_enabled() {
                return false;
            }
            matches!(
                &entry.plugin,
                edgion_resources::resources::edgion_plugins::EdgionPlugin::RequestMirror(config)
                    if config.filter.backend_ref.namespace.as_deref().unwrap_or(namespace) != namespace
            )
        })
    }

    /// Whole-resource last-known-good retention must never carry a JWE
    /// authentication policy, entry condition, or key across generations.
    fn has_enabled_jwe(&self) -> bool {
        self.spec.spec.request_plugins.iter().flatten().any(|entry| {
            entry.is_enabled()
                && matches!(
                    &entry.plugin,
                    edgion_resources::resources::edgion_plugins::EdgionPlugin::JweDecrypt(_)
                )
        })
    }
}

/// Build the compiled runtime for an EdgionPlugins resource.
///
/// Mirrors the former `EdgionPlugins::preparse()`: compile regex conditions on
/// every entry, build the `PluginRuntime` across all four stages, then
/// `seal_fail_closed` so any invalid plugin makes every request through this
/// resource fail closed with HTTP 500.
///
/// Standalone construction for an unscoped resource. Scoped publication uses
/// `PluginGeneration` to share entry construction; neither path runs per request.
pub fn compile_plugins(ep: &EdgionPlugins) -> Arc<CompiledPlugins> {
    compile_plugins_with_cache(ep, &mut EntryCache::default(), &[], Vec::new())
}

/// Compile an effective, equal-length view of one cache-bound source generation.
/// Callers must keep source indices and normalized aliases fixed across masks.
pub(crate) fn compile_plugins_with_cache(
    ep: &EdgionPlugins,
    cache: &mut EntryCache,
    policy_actions: &[(&str, edgion_resources::resources::plugin_policy::PluginPolicyAction)],
    mut errors: Vec<String>,
) -> Arc<CompiledPlugins> {
    let mut spec = ep.clone();
    let mut runtime = PluginRuntime::new();
    let unsupported_errors = ep.unsupported_plugin_errors();
    if !unsupported_errors.is_empty() {
        errors.extend(unsupported_errors);
        runtime.record_unsupported_plugin();
        // Keep entry indices stable for the generation cache, but do not build
        // conditions, dependencies, or filters for non-executable sentinels.
        macro_rules! disable_unsupported {
            ($entries:expr) => {
                for entry in $entries.iter_mut().flatten() {
                    if matches!(entry.plugin, EdgionPlugin::Unsupported) {
                        entry.enable = false;
                    }
                }
            };
        }
        disable_unsupported!(spec.spec.request_plugins);
        disable_unsupported!(spec.spec.upstream_response_filter_plugins);
        disable_unsupported!(spec.spec.upstream_response_body_filter_plugins);
        disable_unsupported!(spec.spec.upstream_response_plugins);
    }

    runtime.configure_request_id_scope(
        ep.has_active_request_id(),
        !ep.validate_request_id_early_attachment().is_empty(),
    );
    let namespace = spec.metadata.namespace.clone().unwrap_or_else(|| "default".to_string());
    // Identity of the resource as the store keys it, so a plugin-entry scope
    // derived from it is unique exactly when the store entry is: re-deriving
    // `ns/name` here instead would normalize a missing namespace to "default"
    // and alias two distinct store entries.
    let resource_key = spec.key_name();

    if let Some(waf) = spec.spec.waf.as_ref() {
        errors.extend(runtime.add_waf_with_cache(waf, cache));
    }

    // Compile regex conditions on all plugin entries once, before building the
    // runtime. This avoids per-request Regex::new() in the fallback path of
    // condition evaluators. add_from_* clones the (now-compiled) conditions into
    // the runtime's ConditionalFilters.
    compile_plugin_conditions(&mut spec, &mut errors, &mut runtime);
    // Mirror the controller's cross-entry ordering rule into the data plane,
    // from the same shared helper and with the same message, so
    // `validate_plugins_matches_compile_plugins_error_set` stays exact. This is
    // not cosmetic: the resource must fail closed even when request conditions
    // would skip the misordered guard and its local ordering check.
    if let Some(entries) = spec.spec.request_plugins.as_ref() {
        let errors_before_ordering = errors.len();
        edgion_resources::resources::edgion_plugins::validate::collect_ai_guard_ordering_errors(entries, &mut errors);
        if errors.len() > errors_before_ordering {
            runtime.record_invalid_ai_guard_ordering();
        }
    }

    // Sanitize `accessLogExtern` declarations: drop any entry `validation_error()`
    // rejects (hard-blocklisted name, empty/over-length key or name) from the
    // *stored* spec before it is Arc-wrapped. `AccessLogExternField` is never a
    // plugin — it has no stage and never runs through `seal_fail_closed` — so this
    // is the only place in the data plane that can enforce the hard blocklist. The
    // controller's admission-time `validation_error()` call (in `preparse`) only
    // reports `Accepted=False`; per the repo's lenient-admission policy the
    // resource is still cached and synced. Without this retain, a hard-blocklisted
    // name (e.g. `authorization`) would reach `apply_declarations` and be logged
    // in clear whenever `access_log_sensitive.mode` is `off` or `wrap`.
    //
    // O(1) per store mutation, zero per-request cost — the alternative (checking
    // `validation_error()` per request in `apply_declarations`) would allocate
    // format strings on the data-plane hot path. Dropping the field (not the
    // request) fails safe: a bad log declaration must not turn into a fail-closed
    // 500 for live traffic, which is why this records an error diagnostic (surfaced only
    // as a status condition) rather than anything `seal_fail_closed` reads.
    if let Some(fields) = spec.spec.access_log_extern.as_mut() {
        let mut idx = 0usize;
        fields.retain(|f| {
            let keep = match f.validation_error() {
                Some(e) => {
                    errors.push(format!("accessLogExtern[{idx}]: {e}"));
                    false
                }
                None => true,
            };
            idx += 1;
            keep
        });
    }

    if let Some(request_plugins) = &spec.spec.request_plugins {
        errors.extend(runtime.add_from_request_filters_with_cache(request_plugins, &namespace, &resource_key, cache));
    }
    if let Some(filter_plugins) = &spec.spec.upstream_response_filter_plugins {
        errors.extend(runtime.add_from_upstream_response_filters_with_cache(filter_plugins, &namespace, cache));
    }
    if let Some(body_filter_plugins) = &spec.spec.upstream_response_body_filter_plugins {
        errors.extend(runtime.add_from_upstream_response_body_filters_with_cache(
            body_filter_plugins,
            &namespace,
            cache,
        ));
    }
    if let Some(response_plugins) = &spec.spec.upstream_response_plugins {
        errors.extend(runtime.add_from_upstream_responses_with_cache(response_plugins, &namespace, cache));
    }

    // Resource-level dye "set clobber" check: a failure rule's response
    // header must not equal (case-insensitively) any success rule's response
    // header across ANY entry in this resource — success renders with Set
    // semantics and would silently wipe an appended failure value queued
    // earlier by a different entry. Runs once over all four stage arrays
    // chained, mirroring the schema-layer `EdgionPlugins::validate_plugins`
    // check (only active, effective enabled entries participate, matching
    // `add_from_*` above).
    let all_dye_suites = spec
        .spec
        .request_plugins
        .iter()
        .flatten()
        .filter(|e| e.is_enabled())
        .map(|e| e.dye.as_ref())
        .chain(
            spec.spec
                .upstream_response_filter_plugins
                .iter()
                .flatten()
                .filter(|e| e.is_enabled())
                .map(|e| e.dye.as_ref()),
        )
        .chain(
            spec.spec
                .upstream_response_body_filter_plugins
                .iter()
                .flatten()
                .filter(|e| e.is_enabled())
                .map(|e| e.dye.as_ref()),
        )
        .chain(
            spec.spec
                .upstream_response_plugins
                .iter()
                .flatten()
                .filter(|e| e.is_enabled())
                .map(|e| e.dye.as_ref()),
        );
    if let Some(err) = edgion_resources::resources::edgion_plugins::dye_set_clobber_error(all_dye_suites) {
        errors.push(err);
    }

    let is_activatable = !runtime.has_invalid_plugins();
    let retain_previous_on_invalid_update =
        runtime.invalid_update_policy() == Some(InvalidUpdatePolicy::RetainPrevious);

    // Fail closed: if any plugin (any stage) had an invalid config, prepend a
    // request-stage RejectFilter so every request through this EdgionPlugins is
    // rejected with 500 before reaching upstream, rather than silently running
    // with the misconfigured plugin absent from the chain.
    runtime.seal_fail_closed();

    for (name, action) in policy_actions {
        runtime.record_policy_action(name, *action);
    }

    // Plugin policy enforcement gates (block ahead of fail-closed reject).
    runtime.seal_plugin_policy();

    // The source generation retains these bundles for future scoped builds.
    // Published views only need the compiled WAF and bounded diagnostics.
    if let Some(waf) = spec.spec.waf.as_mut() {
        waf.resolved_bundles = None;
    }

    Arc::new(CompiledPlugins {
        spec: Arc::new(spec),
        plugin_runtime: Arc::new(runtime),
        preparse_errors: errors,
        is_activatable,
        retain_previous_on_invalid_update,
    })
}

/// Compile regex patterns and validate static config in conditions for all
/// plugin entries. Detailed compile, validation, and unresolved-reference
/// diagnostics are retained in `preparse_errors`; activation-critical failures
/// are also recorded in `PluginRuntime` so `compile_plugins` installs the
/// canonical fail-closed gate.
fn compile_plugin_conditions(ep: &mut EdgionPlugins, errors: &mut Vec<String>, runtime: &mut PluginRuntime) {
    // One entry may fail in its primary gate, secondary body gate, and several
    // dye rules. Preserve every detailed diagnostic, but record its plugin name
    // only once for the eventual RejectFilter access-log entry.
    let mut invalid_entries = HashSet::<(&'static str, usize)>::new();

    macro_rules! compile_entries {
        ($entries:expr, $label:expr) => {
            if let Some(entries) = &mut $entries {
                for (i, entry) in entries.iter_mut().enumerate() {
                    // Entries disabled in the effective scoped view are never built
                    // or run, so dormant conditions must not produce a preparse
                    // error. This stays in lockstep with schema validation.
                    if !entry.is_enabled() {
                        continue;
                    }
                    let mut invalid = false;
                    if let Some(conds) = entry.conditions.as_mut() {
                        if let Err(errs) = conds.compile() {
                            invalid = true;
                            for e in errs {
                                let msg = format!("{}[{}] conditions regex: {}", $label, i, e);
                                tracing::warn!("{}", msg);
                                errors.push(msg);
                            }
                        }
                        let validation = if $label == "requestPlugins"
                            && matches!(
                                entry.plugin,
                                edgion_resources::resources::edgion_plugins::EdgionPlugin::RateLimitRedis(_)
                            ) {
                            conds.validate_for_rate_limit_redis()
                        } else {
                            conds.validate()
                        };
                        if let Err(errs) = validation {
                            invalid = true;
                            for e in errs {
                                let msg = format!("{}[{}] conditions: {}", $label, i, e);
                                tracing::warn!("{}", msg);
                                errors.push(msg);
                            }
                        }
                        if conds.has_unresolved_refs() {
                            let msg = format!("{}[{}] conditions contain unresolved references", $label, i);
                            tracing::warn!("{}", msg);
                            errors.push(msg);
                            invalid = true;
                        }
                    }
                    if invalid && invalid_entries.insert(($label, i)) {
                        runtime.record_invalid_condition(&entry.plugin);
                    }
                }
            }
        };
    }
    compile_entries!(ep.spec.request_plugins, "requestPlugins");
    compile_entries!(
        ep.spec.upstream_response_filter_plugins,
        "upstreamResponseFilterPlugins"
    );
    compile_entries!(
        ep.spec.upstream_response_body_filter_plugins,
        "upstreamResponseBodyFilterPlugins"
    );
    compile_entries!(ep.spec.upstream_response_plugins, "upstreamResponsePlugins");

    // The secondary body gate (`entry.body.conditions`) is a full PluginConditions
    // too — compile its regexes here (only the two async stages carry `body`), or
    // a regex/values_set secondary condition would silently evaluate to false at
    // runtime. Kept in lockstep with `validate.rs` by the drift test.
    macro_rules! compile_body_entries {
        ($entries:expr, $label:expr) => {
            if let Some(entries) = &mut $entries {
                for (i, entry) in entries.iter_mut().enumerate() {
                    // An inert entry's dormant body conditions must not flip
                    // Accepted=False, matching the entry-level conditions rule.
                    if !entry.is_enabled() {
                        continue;
                    }
                    let mut invalid = false;
                    if let Some(conds) = entry.body.as_mut().and_then(|b| b.conditions.as_mut()) {
                        if let Err(errs) = conds.compile() {
                            invalid = true;
                            for e in errs {
                                errors.push(format!("{}[{}] body conditions regex: {}", $label, i, e));
                            }
                        }
                        if let Err(errs) = conds.validate() {
                            invalid = true;
                            for e in errs {
                                errors.push(format!("{}[{}] body conditions: {}", $label, i, e));
                            }
                        }
                        if conds.has_unresolved_refs() {
                            errors.push(format!(
                                "{}[{}] body conditions contain unresolved references",
                                $label, i
                            ));
                            invalid = true;
                        }
                    }
                    if invalid && invalid_entries.insert(($label, i)) {
                        runtime.record_invalid_condition(&entry.plugin);
                    }
                }
            }
        };
    }
    compile_body_entries!(ep.spec.request_plugins, "requestPlugins");
    compile_body_entries!(ep.spec.upstream_response_plugins, "upstreamResponsePlugins");

    // Dye-rule conditions are entry-level (D-27). Compile/validate them here so
    // `compile_plugins` and `validate_plugins` share the same error set.
    macro_rules! compile_dye_entries {
        ($entries:expr, $label:expr) => {
            if let Some(entries) = &mut $entries {
                for (i, entry) in entries.iter_mut().enumerate() {
                    if !entry.is_enabled() {
                        continue;
                    }
                    let mut invalid = false;
                    if let Some(dye) = entry.dye.as_mut() {
                        for rule in dye.request.iter_mut().chain(dye.response.iter_mut()) {
                            if let Some(conds) = rule.conditions.as_mut() {
                                if let Err(errs) = conds.compile() {
                                    invalid = true;
                                    for e in errs {
                                        errors.push(format!("{}[{}] dye conditions regex: {}", $label, i, e));
                                    }
                                }
                                if let Err(errs) = conds.validate() {
                                    invalid = true;
                                    for e in errs {
                                        errors.push(format!("{}[{}] dye conditions: {}", $label, i, e));
                                    }
                                }
                            }
                        }
                    }
                    // Dye references are observational. An unresolved dye
                    // predicate simply does not match and must not invalidate
                    // the entry; only compile/validation failures reach here.
                    if invalid && invalid_entries.insert(($label, i)) {
                        runtime.record_invalid_condition(&entry.plugin);
                    }
                }
            }
        };
    }
    compile_dye_entries!(ep.spec.request_plugins, "requestPlugins");
    compile_dye_entries!(
        ep.spec.upstream_response_filter_plugins,
        "upstreamResponseFilterPlugins"
    );
    compile_dye_entries!(
        ep.spec.upstream_response_body_filter_plugins,
        "upstreamResponseBodyFilterPlugins"
    );
    compile_dye_entries!(ep.spec.upstream_response_plugins, "upstreamResponsePlugins");
}

/// Source inventory and conf-sync adapter. Executable views belong exclusively
/// to the canonical policy publication, and requests access their captured scope.
pub struct PluginStore {
    isolated: Option<Arc<crate::plugins::publication::PluginPublisher>>,
}

impl PluginStore {
    pub(crate) fn publisher(&self) -> &crate::plugins::publication::PluginPublisher {
        self.isolated
            .as_deref()
            .unwrap_or_else(|| crate::plugins::publication::global_publisher())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.publisher().snapshot().http_sources().contains_key(key)
    }

    pub fn count(&self) -> usize {
        self.publisher().snapshot().http_sources().len()
    }

    /// Per-kind plugin entry count across all stored EdgionPlugins resources.
    ///
    /// Discriminator: `EdgionPlugin::type_name()` (enum variant name, e.g. "JwtAuth", "BasicAuth").
    /// Each EdgionPlugins resource may declare plugins across four stages:
    /// requestPlugins, upstreamResponseFilterPlugins, upstreamResponseBodyFilterPlugins,
    /// upstreamResponsePlugins. All stages are counted.
    pub fn count_by_kind(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        let snapshot = self.publisher().snapshot();
        for ep in snapshot.http_sources().values() {
            for entry in ep.request_filter_entries() {
                *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
            }
            for entry in ep.upstream_response_filter_entries() {
                *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
            }
            for entry in ep.upstream_response_body_filter_entries() {
                *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
            }
            for entry in ep.upstream_response_entries() {
                *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
            }
        }
        out
    }

    /// Sample resource keys (namespace/name) holding plugins of the given kind.
    #[allow(dead_code)] // Consumed by validator/sample-diagnostics (Task 15); allow until then.
    pub fn keys_by_kind(&self, kind: &str) -> Vec<String> {
        let mut out = Vec::new();
        let snapshot = self.publisher().snapshot();
        for (key, ep) in snapshot.http_sources().iter() {
            let has_kind = ep.request_filter_entries().iter().any(|e| e.plugin.type_name() == kind)
                || ep
                    .upstream_response_filter_entries()
                    .iter()
                    .any(|e| e.plugin.type_name() == kind)
                || ep
                    .upstream_response_body_filter_entries()
                    .iter()
                    .any(|e| e.plugin.type_name() == kind)
                || ep
                    .upstream_response_entries()
                    .iter()
                    .any(|e| e.plugin.type_name() == kind);
            if has_kind {
                out.push(key.clone());
            }
        }
        out
    }
}

#[cfg(test)]
include!("../../../tests/support/plugins/http/plugin_store_test/519_impl_item.rs");

#[cfg(test)]
include!("../../../tests/support/plugins/http/plugin_store_test/627_impl_item.rs");

#[cfg(test)]
#[path = "../../../tests/unit/plugins/http/plugin_store_test/kind_stats_tests.rs"]
mod kind_stats_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/http/plugin_store_test/access_log_extern_flag_tests.rs"]
mod access_log_extern_flag_tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/http/plugin_store_test/compile_plugins_tests.rs"]
mod compile_plugins_tests;
