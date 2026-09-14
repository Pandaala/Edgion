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
impl PluginStore {
    pub fn new() -> Self {
        let store = Self {
            isolated: Some(Arc::new(crate::plugins::publication::PluginPublisher::new())),
        };
        store.prepare_test_scope(HashSet::new());
        store
    }

    pub(crate) fn prepare_test_scope(&self, keys: HashSet<String>) {
        if self.isolated.is_none() {
            return;
        }
        use crate::plugins::publication::inputs::GatewayInput;
        use edgion_resources::{EdgionGatewayConfig, Gateway, GatewayClass};
        let publisher = self.publisher();
        let mut config: EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
            "apiVersion":"edgion.io/v1", "kind":"EdgionGatewayConfig", "metadata":{"name":"plugin-store-test"},
            "spec":{"globalPluginsRef": keys.iter().map(|key| {
                let (namespace, name) = key.split_once('/').unwrap_or(("default",key));
                serde_json::json!({"namespace":namespace,"name":name})
            }).collect::<Vec<_>>()}
        }))
        .unwrap();
        let class: GatewayClass = serde_json::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"plugin-store-test"},
            "spec":{"controllerName":"edgion.io/gateway-controller","parametersRef":{"group":"edgion.io","kind":"EdgionGatewayConfig","name":"plugin-store-test"}}
        })).unwrap();
        let gateway: Gateway = serde_json::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"Gateway","metadata":{"name":"scope","namespace":"plugin-store-test"},
            "spec":{"gatewayClassName":"plugin-store-test","listeners":[{"name":"http","port":18089,"protocol":"HTTP"}]}
        })).unwrap();
        config.spec.plugin_policy = publisher
            .snapshot()
            .gateway("plugin-store-test/scope")
            .and_then(|gateway| {
                gateway
                    .scope
                    .config
                    .as_ref()
                    .and_then(|config| config.spec.plugin_policy.clone())
            });
        publisher.replace_gateway_configs(HashMap::from([("plugin-store-test".into(), Arc::new(config))]));
        publisher.replace_gateway_classes(HashMap::from([("plugin-store-test".into(), Arc::new(class))]));
        publisher.replace_gateways(HashMap::from([(
            "plugin-store-test/scope".into(),
            GatewayInput::from_gateway(&gateway),
        )]));
    }

    pub(crate) fn scope(&self) -> Arc<crate::plugins::publication::PluginScope> {
        self.publisher()
            .snapshot()
            .gateway("plugin-store-test/scope")
            .unwrap()
            .scope
            .clone()
    }

    pub(crate) fn set_test_policy(
        &self,
        policy: Option<edgion_resources::resources::plugin_policy::PluginPolicyConfig>,
    ) {
        let mut config = self.scope().config.as_ref().unwrap().as_ref().clone();
        config.spec.plugin_policy = policy;
        self.publisher()
            .replace_gateway_configs(HashMap::from([("plugin-store-test".into(), Arc::new(config))]));
    }

    pub fn with_plugin<R>(&self, key: &str, f: impl FnOnce(&CompiledPlugins) -> R) -> Option<R> {
        self.scope().http(key).map(|compiled| f(&compiled))
    }

    pub fn get_plugin_runtime(&self, key: &str) -> Option<Arc<PluginRuntime>> {
        self.with_plugin(key, |compiled| Arc::clone(&compiled.plugin_runtime))
    }

    pub fn replace_all(&self, plugins: HashMap<String, Arc<CompiledPlugins>>) {
        use edgion_common::conf_sync::traits::ConfHandler;
        self.full_set(
            &plugins
                .into_iter()
                .map(|(key, compiled)| (key, Arc::clone(&compiled.spec)))
                .collect(),
        );
    }

    pub(crate) fn apply_delta(
        &self,
        add: HashMap<String, Arc<CompiledPlugins>>,
        update: HashMap<String, Arc<CompiledPlugins>>,
        remove: &HashSet<String>,
    ) {
        use edgion_common::conf_sync::traits::ConfHandler;
        self.partial_update(
            add.into_iter()
                .map(|(key, compiled)| (key, Arc::clone(&compiled.spec)))
                .collect(),
            update
                .into_iter()
                .map(|(key, compiled)| (key, Arc::clone(&compiled.spec)))
                .collect(),
            remove.clone(),
        );
    }
}

#[cfg(test)]
impl Default for PluginStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod kind_stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_empty_breakdown() {
        let store = PluginStore::new();
        let by_kind = store.count_by_kind();
        assert!(by_kind.is_empty());
        let keys = store.keys_by_kind("JwtAuth");
        assert!(keys.is_empty());
    }
}

#[cfg(test)]
mod access_log_extern_flag_tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::plugin_configs::access_log_extern::{
        AccessLogExternField, LogExtSource,
    };
    use edgion_resources::resources::edgion_plugins::EdgionPluginsSpec;
    use edgion_resources::resources::EdgionPlugins;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Barrier;

    fn ep_without_access_log_extern() -> EdgionPlugins {
        EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec::default(),
            status: None,
        }
    }

    fn ep_with_access_log_extern() -> EdgionPlugins {
        EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                access_log_extern: Some(vec![AccessLogExternField {
                    key: "tenant".to_string(),
                    from: LogExtSource::Header,
                    name: "x-tenant".to_string(),
                }]),
                ..Default::default()
            },
            status: None,
        }
    }

    fn assert_snapshot_coherent(store: &PluginStore) {
        let snapshot = store.publisher().snapshot();
        let gateway = snapshot.gateway("plugin-store-test/scope").unwrap();
        assert_eq!(
            gateway.scope.has_access_log_extern,
            gateway
                .scope
                .global_keys
                .iter()
                .filter_map(|key| gateway.scope.http(key))
                .any(|view| view
                    .spec
                    .spec
                    .access_log_extern
                    .as_ref()
                    .is_some_and(|fields| !fields.is_empty()))
        );
    }

    /// The flag must reflect what is currently *stored*, not merely what
    /// entered the store historically: no plugins, plugins without
    /// `accessLogExtern`, then adding one, then removing it.
    #[test]
    fn flag_flips_as_the_store_gains_and_loses_an_access_log_extern_declaration() {
        let store = PluginStore::new();
        assert!(!store.scope().has_access_log_extern, "empty store must report false");
        assert_snapshot_coherent(&store);

        let mut plugins = HashMap::new();
        plugins.insert(
            "ns/no-ext".to_string(),
            compile_plugins(&ep_without_access_log_extern()),
        );
        store.replace_all(plugins);
        assert!(
            !store.scope().has_access_log_extern,
            "a store with plugins but no accessLogExtern must report false"
        );
        assert_snapshot_coherent(&store);

        // Deterministic false-to-true transition.
        let mut add = HashMap::new();
        add.insert("ns/with-ext".to_string(), compile_plugins(&ep_with_access_log_extern()));
        store.apply_delta(add, HashMap::new(), &HashSet::new());
        assert!(
            store.scope().has_access_log_extern,
            "adding a resource with accessLogExtern must flip the flag to true"
        );
        assert_snapshot_coherent(&store);

        // Deterministic true-to-false transition.
        let mut remove = HashSet::new();
        remove.insert("ns/with-ext".to_string());
        store.apply_delta(HashMap::new(), HashMap::new(), &remove);
        assert!(
            !store.scope().has_access_log_extern,
            "removing the only resource with accessLogExtern must flip the flag back to false"
        );
        assert_snapshot_coherent(&store);
    }

    /// `replace_all` is the conf_sync full-sync path: it must SET the flag, not
    /// merely leave it. Without this the `new()` default (false) masks a missing
    /// store in `replace_all`.
    #[test]
    fn replace_all_sets_flag_based_on_content() {
        let store = PluginStore::new();

        // replace_all with a resource declaring accessLogExtern must set the flag.
        let mut full = HashMap::new();
        full.insert("ns/with-ext".to_string(), compile_plugins(&ep_with_access_log_extern()));
        store.replace_all(full);
        assert!(
            store.scope().has_access_log_extern,
            "replace_all with an accessLogExtern resource must set the flag"
        );

        // replace_all with an empty set must clear the flag.
        store.replace_all(HashMap::new());
        assert!(
            !store.scope().has_access_log_extern,
            "replace_all with an empty set must clear the flag"
        );
    }

    #[test]
    fn concurrent_publication_never_exposes_a_mixed_snapshot() {
        const READERS: usize = 4;
        const WRITES: usize = 2_000;
        const READS: usize = 10_000;

        let with_ext = compile_plugins(&ep_with_access_log_extern());
        let without_ext = compile_plugins(&ep_without_access_log_extern());
        let store = Arc::new(PluginStore::new());
        let start = Arc::new(Barrier::new(READERS + 1));
        let failed = Arc::new(AtomicBool::new(false));

        std::thread::scope(|scope| {
            {
                let store = Arc::clone(&store);
                let start = Arc::clone(&start);
                let with_ext = Arc::clone(&with_ext);
                let without_ext = Arc::clone(&without_ext);
                scope.spawn(move || {
                    start.wait();
                    for iteration in 0..WRITES {
                        let generation = if iteration % 2 == 0 {
                            HashMap::from([("ns/with-ext".to_string(), Arc::clone(&with_ext))])
                        } else {
                            HashMap::from([("ns/without-ext".to_string(), Arc::clone(&without_ext))])
                        };
                        store.replace_all(generation);
                    }
                });
            }

            for _ in 0..READERS {
                let store = Arc::clone(&store);
                let start = Arc::clone(&start);
                let failed = Arc::clone(&failed);
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..READS {
                        let snapshot = store.publisher().snapshot();
                        let gateway = snapshot.gateway("plugin-store-test/scope").unwrap();
                        let declares = gateway
                            .scope
                            .global_keys
                            .iter()
                            .filter_map(|key| gateway.scope.http(key))
                            .any(|view| {
                                view.spec
                                    .spec
                                    .access_log_extern
                                    .as_ref()
                                    .is_some_and(|fields| !fields.is_empty())
                            });
                        if gateway.scope.has_access_log_extern != declares {
                            failed.store(true, Ordering::Relaxed);
                        }
                    }
                });
            }
        });

        assert!(
            !failed.load(Ordering::Relaxed),
            "each reader must observe a coherent plugin-store generation"
        );
    }
}

#[cfg(test)]
mod compile_plugins_tests {
    use super::*;
    use crate::plugins::runtime::log::PluginLog;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::filters::PluginRunningResult;
    use edgion_resources::resources::edgion_plugins::conditions::PluginConditions;
    use edgion_resources::resources::edgion_plugins::plugin_configs::access_log_extern::{
        AccessLogExternField, LogExtSource,
    };
    use edgion_resources::resources::edgion_plugins::plugin_configs::DslConfig;
    use edgion_resources::resources::edgion_plugins::{
        BodyRequirement, DyeOutcome, DyeRule, DyeSuite, EdgionPlugin, EdgionPluginsSpec, RequestFilterEntry,
        UpstreamResponseFilterEntry, WafConfig, WafMode,
    };
    use edgion_resources::resources::http_route::{HTTPHeader, HTTPHeaderFilter};
    use edgion_resources::resources::plugin_policy::PluginPolicyAction;

    fn policy_config(
        kind: &str,
        action: PluginPolicyAction,
    ) -> edgion_resources::resources::plugin_policy::PluginPolicyConfig {
        let rule = match action {
            PluginPolicyAction::Bypass => {
                serde_json::json!({"name":format!("http/{kind}"),"action":"bypass"})
            }
            PluginPolicyAction::Block { status } => {
                serde_json::json!({"name":format!("http/{kind}"),"blockStatus":status})
            }
        };
        serde_json::from_value(serde_json::json!({"deny":[rule]})).unwrap()
    }

    fn compile_with_policy(
        resource: &EdgionPlugins,
        kind: &str,
        action: Option<PluginPolicyAction>,
    ) -> Arc<CompiledPlugins> {
        let policy = action.map(|action| policy_config(kind, action).compile().unwrap());
        super::super::plugin_generation::PluginGeneration::new(resource.clone()).compile(policy.as_ref())
    }

    fn first_filter(runtime: &PluginRuntime) -> &Arc<dyn crate::plugins::runtime::RequestFilter> {
        runtime.request_plugins_iter().next().unwrap()
    }

    fn ai_ordering_resource(types: &[&str]) -> EdgionPlugins {
        let entries: Vec<serde_json::Value> = types
            .iter()
            .map(|kind| {
                let config = if *kind == "AiGuard" {
                    serde_json::json!({"rules": [{"name": "guard-rule",
                        "match": {"type": "regex", "pattern": "private-prompt"}, "action": "reject"}]})
                } else {
                    serde_json::json!({})
                };
                serde_json::json!({"type": kind, "config": config})
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
            "metadata": {"name": "ai-ordering", "namespace": "default"},
            "spec": {"requestPlugins": entries}
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn ai_ordering_invalid_generation_rejects_once_before_request_plugins() {
        for types in [
            vec!["AiProxy", "AiGuard"],
            vec!["AiProxy", "AiGuard", "AiGuard"],
            vec!["AiGuard", "AiProxy", "AiGuard"],
        ] {
            let mut resource = ai_ordering_resource(&types);
            // Even a guard that would skip this request cannot bypass the
            // unconditional resource-level configuration gate.
            resource
                .spec
                .request_plugins
                .as_mut()
                .unwrap()
                .last_mut()
                .unwrap()
                .conditions = Some(conditions(serde_json::json!({
                    "skip": {"allOf": [{"type": "probability", "ratio": 1.0}]}
            })));
            let compiled = compile_plugins(&resource);
            assert_eq!(compiled.preparse_errors, resource.validate_plugins());
            assert_eq!(compiled.preparse_errors.len(), 1);
            assert!(!compiled.is_activatable);
            assert!(!compiled.retain_previous_on_invalid_update);
            assert_leading_reject(&compiled);
            assert_eq!(
                compiled
                    .plugin_runtime
                    .request_plugins_iter()
                    .filter(|filter| filter.name() == "RejectInvalidConfig")
                    .count(),
                1
            );

            let reject = compiled.plugin_runtime.request_plugins_iter().next().unwrap();
            // No session access is permitted: the reject must precede body
            // reads, condition evaluation, evaluator calls and AI planning.
            let mut session = MockPluginSession::new();
            let mut log = PluginLog::new(reject.name());
            let result = reject.run_request(&mut session, &mut log).await;
            match result {
                PluginRunningResult::ErrResponse { status, body } => {
                    assert_eq!(status, 500);
                    assert_eq!(body.unwrap().as_ref(), br#"{"message":"plugin configuration error"}"#);
                }
                other => panic!("expected fail-closed response, got {other:?}"),
            }
            let serialized = serde_json::to_value(&log).unwrap();
            let entries = serialized["log"].as_array().unwrap();
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.as_str() == Some("err: AiGuardOrdering"))
                    .count(),
                1
            );
            assert!(!serialized.to_string().contains("private-prompt"));
        }
    }

    #[test]
    fn ai_ordering_inert_entries_and_valid_orders_do_not_seal() {
        for types in [vec!["AiGuard", "AiProxy"], vec!["AiGuard"], vec!["AiProxy"]] {
            let resource = ai_ordering_resource(&types);
            let compiled = compile_plugins(&resource);
            assert_eq!(compiled.preparse_errors, resource.validate_plugins());
            assert!(compiled.preparse_errors.is_empty());
            assert!(compiled.is_activatable);
        }
        for index in 0..2 {
            for (enabled, policy) in [
                (false, None),
                (true, Some(PluginPolicyAction::Bypass)),
                (true, Some(PluginPolicyAction::Block { status: 403 })),
            ] {
                let mut resource = ai_ordering_resource(&["AiProxy", "AiGuard"]);
                let entry = &mut resource.spec.request_plugins.as_mut().unwrap()[index];
                entry.enable = enabled;
                let kind = entry.plugin.type_name();
                let compiled = compile_with_policy(&resource, kind, policy);
                if policy.is_none() {
                    assert_eq!(compiled.preparse_errors, resource.validate_plugins());
                }
                assert!(compiled.preparse_errors.is_empty());
                assert!(compiled.is_activatable);
                assert!(compiled
                    .plugin_runtime
                    .request_plugins_iter()
                    .all(|filter| filter.name() != "RejectInvalidConfig"));
            }
        }
    }

    #[test]
    fn ai_ordering_invalid_publications_replace_and_recover() {
        let valid = compile_plugins(&ai_ordering_resource(&["AiGuard", "AiProxy"]));
        let invalid = compile_plugins(&ai_ordering_resource(&["AiProxy", "AiGuard"]));
        let key = "default/ai-ordering".to_string();
        for mode in ["full", "update", "add"] {
            let store = PluginStore::new();
            // Include invalid first publication, recovery, valid-to-invalid,
            // repeated invalid publication, and final recovery.
            for incoming in [&invalid, &valid, &invalid, &invalid, &valid] {
                let map = HashMap::from([(key.clone(), Arc::clone(incoming))]);
                match mode {
                    "full" => store.replace_all(map),
                    "update" => store.apply_delta(HashMap::new(), map, &HashSet::new()),
                    "add" => store.apply_delta(map, HashMap::new(), &HashSet::new()),
                    _ => unreachable!(),
                }
                let snapshot = store.scope();
                let published = snapshot.http(&key).unwrap();
                assert_eq!(published.preparse_errors, incoming.preparse_errors, "{mode}");
                assert_eq!(published.is_activatable, Arc::ptr_eq(incoming, &valid));
                if !published.is_activatable {
                    assert_leading_reject(&published);
                }
            }
        }
    }

    #[tokio::test]
    async fn ai_ordering_invalid_generation_preserves_policy_block_precedence() {
        let mut resource = ai_ordering_resource(&["AiProxy", "AiGuard"]);
        resource
            .spec
            .request_plugins
            .as_mut()
            .unwrap()
            .push(RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
                HTTPHeaderFilter {
                    set: None,
                    add: None,
                    remove: None,
                },
            )));
        let compiled = compile_with_policy(
            &resource,
            "RequestHeaderModifier",
            Some(PluginPolicyAction::Block { status: 403 }),
        );
        assert_eq!(compiled.preparse_errors, resource.validate_plugins());
        assert_eq!(compiled.preparse_errors.len(), 1);
        assert!(!compiled.is_activatable);
        assert!(!compiled.retain_previous_on_invalid_update);
        let mut filters = compiled.plugin_runtime.request_plugins_iter();
        let policy = filters.next().unwrap();
        assert_eq!(policy.name(), "PluginPolicyBlock");
        assert_eq!(filters.next().unwrap().name(), "RejectInvalidConfig");
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(policy.name());
        assert!(matches!(
            policy.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { status: 403, .. }
        ));
    }

    #[test]
    fn ai_ordering_failure_overrides_dsl_and_waf_retention() {
        for program in ["dsl", "waf"] {
            let mut resource = ai_ordering_resource(&["AiGuard", "AiProxy"]);
            if program == "dsl" {
                resource
                    .spec
                    .request_plugins
                    .as_mut()
                    .unwrap()
                    .push(RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                        source: Some("invalid @@@".to_string()),
                        ..Default::default()
                    })));
            } else {
                resource.spec.waf = waf_resource("SecRule invalid-(").spec.waf;
            }
            let program_only = compile_plugins(&resource);
            assert!(!program_only.is_activatable);
            assert!(program_only.retain_previous_on_invalid_update);
            resource.spec.request_plugins.as_mut().unwrap().swap(0, 1);
            let mixed = compile_plugins(&resource);
            let mut expected = resource.validate_plugins();
            expected.sort();
            let mut actual = mixed.preparse_errors.clone();
            actual.sort();
            assert_eq!(actual, expected);
            assert!(!mixed.is_activatable);
            assert!(!mixed.retain_previous_on_invalid_update);
            assert_leading_reject(&mixed);
            let valid = compile_plugins(&ai_ordering_resource(&["AiGuard", "AiProxy"]));
            assert!(!mixed.can_retain_previous(&valid));
        }
    }

    #[test]
    fn dye_stage_policy_and_inert_entries_have_controller_gateway_parity() {
        use edgion_resources::resources::edgion_plugins::{
            DyeStage, UpstreamResponseBodyFilterEntry, UpstreamResponseEntry,
        };
        let file = tempfile::Builder::new().suffix(".wasm").tempfile().unwrap();
        std::fs::write(file.path(), wat::parse_str("(module)").unwrap()).unwrap();
        for stage in [
            DyeStage::Request,
            DyeStage::UpstreamResponseFilter,
            DyeStage::UpstreamResponseBodyFilter,
            DyeStage::UpstreamResponse,
        ] {
            for fail_open in [false, true] {
                for outcome in [DyeOutcome::FailOpen, DyeOutcome::FailClose] {
                    // Disabled, bypassed and blocked entries remain inert for
                    // dye validation; a policy block still owns its existing denial.
                    for (enable, policy) in [
                        (true, None),
                        (false, None),
                        (true, Some(PluginPolicyAction::Bypass)),
                        (true, Some(PluginPolicyAction::Block { status: 403 })),
                    ] {
                        let plugin: EdgionPlugin = serde_json::from_value(serde_json::json!({
                            "type": "Wasm", "config": { "source": { "file": file.path() }, "failOpen": fail_open }
                        }))
                        .unwrap();
                        let dye = Some(DyeSuite {
                            request: vec![],
                            response: vec![DyeRule {
                                name: "X-Dye".into(),
                                on: vec![outcome],
                                value: None,
                                conditions: None,
                            }],
                        });
                        let active = enable && policy.is_none();
                        let supported = matches!(stage, DyeStage::Request | DyeStage::UpstreamResponseFilter)
                            && (outcome == DyeOutcome::FailOpen) == fail_open;
                        let mut resource = EdgionPlugins::new("dye", EdgionPluginsSpec::default());
                        macro_rules! entry {
                            ($ty:ident, $field:ident) => {{
                                let mut entry = $ty::new(plugin);
                                entry.enable = enable;
                                entry.dye = dye;
                                resource.spec.$field = Some(vec![entry]);
                            }};
                        }
                        match stage {
                            DyeStage::Request => entry!(RequestFilterEntry, request_plugins),
                            DyeStage::UpstreamResponseFilter => {
                                entry!(UpstreamResponseFilterEntry, upstream_response_filter_plugins)
                            }
                            DyeStage::UpstreamResponseBodyFilter => {
                                entry!(UpstreamResponseBodyFilterEntry, upstream_response_body_filter_plugins)
                            }
                            DyeStage::UpstreamResponse => {
                                entry!(UpstreamResponseEntry, upstream_response_plugins)
                            }
                        }
                        let source_errors = resource.validate_plugins();
                        let compiled = compile_with_policy(&resource, "Wasm", policy);
                        if policy.is_none() {
                            assert_eq!(
                                source_errors, compiled.preparse_errors,
                                "{stage:?} {outcome:?} {fail_open}"
                            );
                        }
                        let schema = &compiled.preparse_errors;
                        assert_eq!(
                            schema
                                .iter()
                                .any(|e| e.contains("cannot render in this stage and configuration")),
                            active && !supported
                        );
                        let has_plan = match stage {
                            DyeStage::Request => compiled
                                .plugin_runtime
                                .request_plugins_iter()
                                .any(|p| p.dye_plan().is_some()),
                            DyeStage::UpstreamResponseFilter => compiled
                                .plugin_runtime
                                .upstream_response_plugins_iter()
                                .any(|p| p.dye_plan().is_some()),
                            DyeStage::UpstreamResponseBodyFilter => compiled
                                .plugin_runtime
                                .upstream_response_body_plugins_iter()
                                .any(|p| p.dye_plan().is_some()),
                            DyeStage::UpstreamResponse => compiled
                                .plugin_runtime
                                .upstream_response_async_plugins_iter()
                                .any(|p| p.dye_plan().is_some()),
                        };
                        assert_eq!(has_plan, active && supported);
                        if active && stage != DyeStage::UpstreamResponse {
                            assert!(
                                compiled.is_activatable,
                                "a dye error must not disable the plugin itself"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn dye_policy_update_replaces_plan_and_recovers_without_retaining_old_rules() {
        use edgion_resources::resources::edgion_plugins::plugin_configs::DslErrorPolicy;
        let mut entry = RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "dye".into(),
            source: Some("return next()".into()),
            error_policy: Some(DslErrorPolicy::Deny),
            ..Default::default()
        }));
        entry.dye = Some(DyeSuite {
            request: vec![],
            response: vec![DyeRule {
                name: "X-Dye".into(),
                on: vec![DyeOutcome::FailClose],
                value: None,
                conditions: None,
            }],
        });
        let mut resource = dsl_resource(entry);
        let store = PluginStore::new();
        let key = "default/dye".to_string();
        for outcome in [DyeOutcome::FailClose, DyeOutcome::FailOpen, DyeOutcome::FailClose] {
            resource.spec.request_plugins.as_mut().unwrap()[0]
                .dye
                .as_mut()
                .unwrap()
                .response[0]
                .on = vec![outcome];
            let compiled = compile_plugins(&resource);
            assert_eq!(resource.validate_plugins(), compiled.preparse_errors);
            assert!(compiled.is_activatable, "{:?}", compiled.preparse_errors);
            assert!(!compiled.retain_previous_on_invalid_update);
            store.apply_delta(
                HashMap::new(),
                HashMap::from([(key.clone(), compiled)]),
                &HashSet::new(),
            );
            assert_eq!(
                store.with_plugin(&key, |p| p
                    .plugin_runtime
                    .request_plugins_iter()
                    .any(|f| f.dye_plan().is_some())),
                Some(outcome == DyeOutcome::FailClose)
            );
        }
    }

    #[test]
    fn dye_canary_internal_kill_switch_keeps_anti_spoof_plan() {
        let mut resource: EdgionPlugins = serde_yaml::from_str(include_str!(
            "../../../../edgion-tests/integration/conf/EdgionPlugins/Canary/20_EdgionPlugins_default_canary-kill-switch-strip.yaml"
        )).unwrap();
        let compiled = compile_plugins(&resource);
        assert!(resource.validate_plugins().is_empty());
        assert!(compiled.preparse_errors.is_empty());
        let plan = compiled
            .plugin_runtime
            .request_plugins_iter()
            .next()
            .unwrap()
            .dye_plan()
            .unwrap();
        assert_eq!(plan.request[0].name, "X-Canary-Active");
        // The outer switch is different: no entry, no plan and no stripping.
        resource.spec.request_plugins.as_mut().unwrap()[0].enable = false;
        let compiled = compile_plugins(&resource);
        assert_eq!(compiled.plugin_runtime.request_plugins_count(), 0);
    }

    #[test]
    fn dye_named_jump_fixtures_reject_unreachable_subscriptions_consistently() {
        // The integration manifests now subscribe only to supported outcomes.
        // Restore broad subscriptions in memory to retain D0083 negative coverage.
        let fixtures = [
            (false, include_str!("../../../../edgion-tests/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-canary-fallback.yaml")),
            (false, include_str!("../../../../edgion-tests/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-canary-reject.yaml")),
            (false, include_str!("../../../../edgion-tests/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-diu-fallback.yaml")),
            (true, include_str!("../../../../edgion-tests/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-diu-reject.yaml")),
        ];
        for (valid, yaml) in fixtures {
            let mut resource: EdgionPlugins = serde_yaml::from_str(yaml).unwrap();
            let fixture = compile_plugins(&resource);
            assert!(resource.validate_plugins().is_empty());
            assert!(fixture.preparse_errors.is_empty());
            assert!(fixture.is_activatable);
            assert!(fixture
                .plugin_runtime
                .request_plugins_iter()
                .any(|plugin| plugin.dye_plan().is_some()));
            if !valid {
                resource.spec.request_plugins.as_mut().unwrap()[0]
                    .dye
                    .as_mut()
                    .unwrap()
                    .response[0]
                    .on = vec![DyeOutcome::Reject, DyeOutcome::Fallback];
            }
            let errors = resource.validate_plugins();
            let compiled = compile_plugins(&resource);
            assert_eq!(errors, compiled.preparse_errors);
            assert_eq!(errors.is_empty(), valid, "{:?}: {errors:?}", resource.metadata.name);
            if !valid {
                assert!(errors
                    .iter()
                    .any(|e| e.contains("cannot render in this stage and configuration")));
            }
            assert!(compiled.is_activatable, "only dye is rejected");
            assert_eq!(
                compiled
                    .plugin_runtime
                    .request_plugins_iter()
                    .any(|p| p.dye_plan().is_some()),
                valid
            );
        }
    }

    fn header_modifier() -> EdgionPlugin {
        EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
            set: Some(vec![HTTPHeader {
                name: "X-Test".into(),
                value: "test-value".into(),
            }]),
            add: None,
            remove: None,
        })
    }

    fn conditions(value: serde_json::Value) -> PluginConditions {
        serde_json::from_value(value).expect("condition fixture must deserialize")
    }

    fn resource_with_ip_conditions(count: usize) -> EdgionPlugins {
        use edgion_resources::resources::edgion_plugins::conditions::{Condition, ConditionSet, IpAccessMatch};

        let mut entry = RequestFilterEntry::new(header_modifier());
        entry.conditions = Some(PluginConditions {
            run: Some(ConditionSet::all_of(vec![
                Condition::DirectPeerIpMatch(IpAccessMatch {
                    ips: vec!["192.0.2.0/24".to_string()],
                    ..Default::default()
                });
                count
            ])),
            ..Default::default()
        });
        EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![entry]),
                ..Default::default()
            },
            status: None,
        }
    }

    #[test]
    fn ip_condition_count_does_not_limit_validation_or_publication() {
        let key = "default/ip-conditions".to_string();
        let store = PluginStore::new();
        let initial = compile_plugins(&resource_with_ip_conditions(1));
        store.replace_all(HashMap::from([(key.clone(), initial)]));

        // Keep literal historical boundaries independent of consumer-side limits.
        for count in [10_000, 10_001] {
            let resource = resource_with_ip_conditions(count);
            let errors = resource.validate_plugins();
            assert!(errors.is_empty(), "{count}: {errors:?}");
            let compiled = compile_plugins(&resource);
            assert!(compiled.preparse_errors.is_empty(), "{:?}", compiled.preparse_errors);
            assert!(compiled.is_activatable);
            assert!(!compiled.retain_previous_on_invalid_update);
            assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);
            assert!(compiled
                .plugin_runtime
                .request_plugins_iter()
                .all(|filter| filter.name() != "RejectInvalidConfig"));

            store.apply_delta(
                HashMap::new(),
                HashMap::from([(key.clone(), Arc::clone(&compiled))]),
                &HashSet::new(),
            );
            assert!(store
                .with_plugin(&key, |published| published.is_activatable
                    && published.preparse_errors.is_empty()
                    && published.plugin_runtime.request_plugins_count() == 1)
                .unwrap());
            store.replace_all(HashMap::from([(key.clone(), Arc::clone(&compiled))]));
            assert!(store
                .with_plugin(&key, |published| published.is_activatable
                    && published.preparse_errors.is_empty()
                    && published.plugin_runtime.request_plugins_count() == 1)
                .unwrap());
        }
    }

    #[test]
    fn large_ip_condition_resource_still_rejects_invalid_conditions() {
        let mut resource = resource_with_ip_conditions(10_001);
        resource.spec.request_plugins.as_mut().unwrap()[0]
            .conditions
            .as_mut()
            .unwrap()
            .skip = malformed_time_range_conditions().run;
        let errors = resource.validate_plugins();
        assert!(!errors.is_empty());
        let compiled = compile_plugins(&resource);
        assert_eq!(compiled.preparse_errors, errors);
        assert!(!compiled.is_activatable);
        assert!(!compiled.retain_previous_on_invalid_update);
        assert_leading_reject(&compiled);
    }

    fn malformed_time_range_conditions() -> PluginConditions {
        conditions(serde_json::json!({
            "run": { "allOf": [{ "type": "timeRange", "after": "not-rfc3339" }] }
        }))
    }

    fn bad_regex_conditions() -> PluginConditions {
        conditions(serde_json::json!({
            "run": { "allOf": [{
                "type": "keyMatch",
                "key": { "type": "header", "name": "X-Test" },
                "regex": ["("]
            }] }
        }))
    }

    fn unresolved_conditions() -> PluginConditions {
        conditions(serde_json::json!({
            "run": { "allOf": [{
                "type": "remoteIpMatch",
                "ipRefs": [{ "name": "missing" }],
                "negate": true
            }] }
        }))
    }

    fn body_dsl_entry() -> RequestFilterEntry {
        RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "body-condition-test".to_string(),
            source: Some("let found = req.body.contains(\"needle\")".to_string()),
            ..Default::default()
        }))
    }

    fn dsl_resource(entry: RequestFilterEntry) -> EdgionPlugins {
        EdgionPlugins {
            metadata: kube::api::ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("condition-test".to_string()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![entry]),
                ..Default::default()
            },
            status: None,
        }
    }

    fn response_dye(conditions: Vec<PluginConditions>) -> DyeSuite {
        DyeSuite {
            request: Vec::new(),
            response: conditions
                .into_iter()
                .enumerate()
                .map(|(index, conditions)| DyeRule {
                    name: format!("X-Condition-Dye-{index}"),
                    on: vec![DyeOutcome::FailOpen, DyeOutcome::FailClose],
                    value: None,
                    conditions: Some(conditions),
                })
                .collect(),
        }
    }

    fn entry_condition_resource(condition: PluginConditions) -> EdgionPlugins {
        let mut entry = body_dsl_entry();
        entry.conditions = Some(condition);
        dsl_resource(entry)
    }

    fn body_condition_resource(condition: PluginConditions) -> EdgionPlugins {
        let mut entry = body_dsl_entry();
        entry.body = Some(BodyRequirement {
            conditions: Some(condition),
            ..Default::default()
        });
        dsl_resource(entry)
    }

    fn dye_condition_resource(condition: PluginConditions) -> EdgionPlugins {
        let mut entry = body_dsl_entry();
        entry.dye = Some(response_dye(vec![condition]));
        dsl_resource(entry)
    }

    fn assert_leading_reject(compiled: &CompiledPlugins) {
        assert_eq!(
            compiled
                .plugin_runtime
                .request_plugins_iter()
                .next()
                .map(|filter| filter.name()),
            Some("RejectInvalidConfig")
        );
    }

    #[test]
    fn active_condition_error_matrix_seals_fail_closed() {
        let cases = [
            (
                "entry timeRange",
                entry_condition_resource(malformed_time_range_conditions()),
                "conditions: timeRange.after",
            ),
            (
                "entry regex",
                entry_condition_resource(bad_regex_conditions()),
                "conditions regex:",
            ),
            (
                "body timeRange",
                body_condition_resource(malformed_time_range_conditions()),
                "body conditions: timeRange.after",
            ),
            (
                "body regex",
                body_condition_resource(bad_regex_conditions()),
                "body conditions regex:",
            ),
            (
                "dye timeRange",
                dye_condition_resource(malformed_time_range_conditions()),
                "dye conditions: timeRange.after",
            ),
            (
                "dye regex",
                dye_condition_resource(bad_regex_conditions()),
                "dye conditions regex:",
            ),
        ];

        for (name, resource, expected_error) in cases {
            let mut controller_errors = resource.validate_plugins();
            controller_errors.sort();
            let compiled = compile_plugins(&resource);
            let mut gateway_errors = compiled.preparse_errors.clone();
            gateway_errors.sort();

            assert_eq!(gateway_errors, controller_errors, "static validation parity: {name}");
            assert!(
                compiled
                    .preparse_errors
                    .iter()
                    .any(|error| error.contains(expected_error)),
                "missing {expected_error:?} for {name}: {:?}",
                compiled.preparse_errors
            );
            assert!(!compiled.is_activatable, "{name}");
            assert!(!compiled.retain_previous_on_invalid_update, "{name}");
            assert_leading_reject(&compiled);
        }
    }

    #[test]
    fn unresolved_body_condition_is_nonactivatable_nonretainable_and_sealed() {
        let compiled = compile_plugins(&body_condition_resource(unresolved_conditions()));

        assert!(compiled
            .preparse_errors
            .iter()
            .any(|error| { error == "requestPlugins[0] body conditions contain unresolved references" }));
        assert!(!compiled.is_activatable);
        assert!(!compiled.retain_previous_on_invalid_update);
        assert_leading_reject(&compiled);
    }

    #[test]
    fn unresolved_dye_condition_remains_observational_while_malformed_dye_is_invalid() {
        let unresolved = compile_plugins(&dye_condition_resource(unresolved_conditions()));
        assert!(unresolved.is_activatable);
        assert!(unresolved.preparse_errors.is_empty());
        assert!(unresolved
            .plugin_runtime
            .request_plugins_iter()
            .all(|filter| filter.name() != "RejectInvalidConfig"));

        for resource in [
            dye_condition_resource(malformed_time_range_conditions()),
            dye_condition_resource(bad_regex_conditions()),
        ] {
            let compiled = compile_plugins(&resource);
            assert!(!compiled.is_activatable);
            assert!(!compiled.retain_previous_on_invalid_update);
            assert_leading_reject(&compiled);
        }
    }

    #[test]
    fn dsl_condition_failure_replaces_last_known_good_generation() {
        let key = "default/condition-test".to_string();
        let valid_entry = body_dsl_entry();
        let mut invalid_entry = valid_entry.clone();
        invalid_entry.conditions = Some(malformed_time_range_conditions());
        let valid = compile_plugins(&dsl_resource(valid_entry));
        let invalid = compile_plugins(&dsl_resource(invalid_entry));
        assert!(valid.is_activatable);
        assert!(!invalid.is_activatable);
        assert!(!invalid.retain_previous_on_invalid_update);
        assert_leading_reject(&invalid);

        let store = PluginStore::new();
        store.apply_delta(
            HashMap::from([(key.clone(), Arc::clone(&valid))]),
            HashMap::new(),
            &HashSet::new(),
        );
        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::clone(&invalid))]),
            &HashSet::new(),
        );

        let snapshot = store.scope();
        let published = snapshot.http(&key).expect("updated generation is published");
        assert!(!published.is_activatable);
        assert_eq!(published.preparse_errors, invalid.preparse_errors);
        assert_leading_reject(&published);
    }

    #[tokio::test]
    async fn condition_invalid_entry_is_logged_once_across_all_condition_sites() {
        let mut entry = body_dsl_entry();
        entry.conditions = Some(malformed_time_range_conditions());
        entry.body = Some(BodyRequirement {
            conditions: Some(bad_regex_conditions()),
            ..Default::default()
        });
        entry.dye = Some(response_dye(vec![
            malformed_time_range_conditions(),
            bad_regex_conditions(),
        ]));
        let compiled = compile_plugins(&dsl_resource(entry));

        for expected in [
            "conditions: timeRange.after",
            "body conditions regex:",
            "dye conditions: timeRange.after",
            "dye conditions regex:",
        ] {
            assert!(
                compiled.preparse_errors.iter().any(|error| error.contains(expected)),
                "missing {expected:?}: {:?}",
                compiled.preparse_errors
            );
        }
        assert_leading_reject(&compiled);

        let reject = compiled
            .plugin_runtime
            .request_plugins_iter()
            .next()
            .expect("reject gate is installed");
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(reject.name());
        let result = reject.run_request(&mut session, &mut log).await;
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));

        let serialized = serde_json::to_value(&log).expect("PluginLog serializes");
        let entries = serialized["log"].as_array().expect("reject log entries");
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.as_str() == Some("err: invalid plugin config"))
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.as_str() == Some("err: Dsl"))
                .count(),
            1
        );
    }

    #[test]
    fn disabled_and_scoped_bypass_malformed_conditions_remain_inert() {
        for (name, mut entry) in [("disabled", body_dsl_entry()), ("policy bypass", body_dsl_entry())] {
            entry.conditions = Some(malformed_time_range_conditions());
            entry.body = Some(BodyRequirement {
                conditions: Some(bad_regex_conditions()),
                ..Default::default()
            });
            entry.dye = Some(response_dye(vec![malformed_time_range_conditions()]));
            if name == "disabled" {
                entry.enable = false;
            }

            let resource = dsl_resource(entry);
            let action = (name != "disabled").then_some(PluginPolicyAction::Bypass);
            assert_eq!(resource.validate_plugins().is_empty(), name == "disabled");
            let compiled = compile_with_policy(&resource, "Dsl", action);
            assert!(
                compiled.preparse_errors.is_empty(),
                "{name}: {:?}",
                compiled.preparse_errors
            );
            assert!(compiled.is_activatable, "{name}");
            assert!(compiled
                .plugin_runtime
                .request_plugins_iter()
                .all(|filter| filter.name() != "RejectInvalidConfig"));
        }
    }

    #[test]
    fn auth_realm_invalid_configs_match_controller_validation_and_fail_closed() {
        for kind in [
            "BasicAuth",
            "KeyAuth",
            "JwtAuth",
            "HmacAuth",
            "LdapAuth",
            "OpenidConnect",
        ] {
            let mut config = match kind {
                "LdapAuth" => serde_json::json!({
                    "ldapHost": "ldap.example.com", "baseDn": "dc=example,dc=com", "attribute": "uid"
                }),
                "OpenidConnect" => serde_json::json!({
                    "discovery": "https://idp.example.com/.well-known/openid-configuration",
                    "clientId": "test-client", "bearerOnly": true
                }),
                _ => serde_json::json!({"anonymous": "guest"}),
            };
            config["realm"] = serde_json::json!("private\r\nrealm");
            let ep: EdgionPlugins = serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
                "metadata": {"name": "realm-test", "namespace": "default"},
                "spec": {"requestPlugins": [{"type": kind, "config": config}]}
            }))
            .unwrap();
            let controller_errors = ep.validate_plugins();
            assert!(!controller_errors.is_empty(), "{kind}");
            let compiled = compile_plugins(&ep);
            assert_eq!(compiled.preparse_errors, controller_errors, "{kind}");
            assert!(!compiled.is_activatable, "{kind}");
            assert!(!compiled.retain_previous_on_invalid_update, "{kind}");
            assert_eq!(
                compiled.plugin_runtime.request_plugins_count(),
                1,
                "fail-closed gate: {kind}"
            );
            assert!(!compiled.preparse_errors.join(";").contains("private"));
        }
    }

    #[test]
    fn auth_realm_invalid_reload_replaces_valid_generation_and_recovers() {
        let resource = |realm: &str, enabled: bool| -> EdgionPlugins {
            serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
                "metadata": {"name": "realm-test", "namespace": "default"},
                "spec": {"requestPlugins": [{"type": "BasicAuth", "enable": enabled,
                    "config": {"anonymous": "guest", "realm": realm}}]}
            }))
            .unwrap()
        };
        let store = PluginStore::new();
        let key = "default/realm-test".to_string();
        let valid = compile_plugins(&resource("initial", true));
        assert!(valid.is_activatable);
        store.replace_all(HashMap::from([(key.clone(), valid)]));
        for (realm, expected_active) in [("bad\0realm", false), ("Partner \"API\"\\", true)] {
            let next = compile_plugins(&resource(realm, true));
            assert_eq!(next.is_activatable, expected_active);
            store.apply_delta(
                HashMap::new(),
                HashMap::from([(key.clone(), Arc::clone(&next))]),
                &HashSet::new(),
            );
            let published = store.scope().http(&key).unwrap();
            assert_eq!(published.is_activatable, expected_active);
            assert_eq!(published.preparse_errors, next.preparse_errors);
        }
        let disabled = resource("bad\0realm", false);
        assert!(disabled.validate_plugins().is_empty());
        assert!(compile_plugins(&disabled).is_activatable);
    }

    #[test]
    fn header_modifier_invalid_reload_replaces_valid_generation_and_recovers() {
        for (stage, kind) in [
            ("requestPlugins", "RequestHeaderModifier"),
            ("upstreamResponseFilterPlugins", "ResponseHeaderModifier"),
        ] {
            let resource = |value: &str| -> EdgionPlugins {
                serde_json::from_value(serde_json::json!({
                    "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
                    "metadata": {"name": "headers", "namespace": "default"},
                    "spec": {stage: [{"type": kind, "config": {
                        "set": [{"name": "x-test", "value": value}]
                    }}]}
                }))
                .unwrap()
            };
            let store = PluginStore::new();
            let key = "default/headers".to_string();
            let initial = compile_plugins(&resource("initial"));
            assert!(initial.is_activatable);
            store.replace_all(HashMap::from([(key.clone(), initial)]));
            let captured = store.scope().http(&key).unwrap();
            for (value, expected_active) in [("private\r\nvalue", false), ("recovered", true)] {
                let source = resource(value);
                let next = compile_plugins(&source);
                assert_eq!(next.preparse_errors, source.validate_plugins());
                assert_eq!(next.is_activatable, expected_active);
                assert!(!next.retain_previous_on_invalid_update);
                store.apply_delta(HashMap::new(), HashMap::from([(key.clone(), next)]), &HashSet::new());
                let published = store.scope().http(&key).unwrap();
                assert_eq!(published.is_activatable, expected_active);
                if !expected_active {
                    assert_leading_reject(&published);
                    assert!(!published.preparse_errors.join(";").contains("private"));
                }
                // An already captured generation remains valid through reload.
                assert!(captured.is_activatable);
            }
        }
    }

    fn waf_resource(rules: &str) -> EdgionPlugins {
        EdgionPlugins {
            metadata: kube::api::ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("native-waf".to_string()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
                waf: Some(WafConfig {
                    rules: rules.to_string(),
                    mode: WafMode::On,
                    priority: 0,
                }),
                ..Default::default()
            },
            status: None,
        }
    }

    #[test]
    fn valid_waf_builds_one_logical_filter_for_each_supported_gateway_hook() {
        let compiled = compile_plugins(&waf_resource(
            "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny,status:403\"\n",
        ));

        assert!(compiled.is_activatable);
        assert!(compiled.preparse_errors.is_empty());
        assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);
        assert_eq!(compiled.plugin_runtime.upstream_response_plugins_count(), 1);
        assert_eq!(compiled.plugin_runtime.upstream_response_body_plugins_count(), 0);
        assert_eq!(compiled.plugin_runtime.upstream_response_async_plugins_count(), 1);
    }

    #[test]
    fn invalid_waf_update_retains_the_last_known_good_generation() {
        let valid = compile_plugins(&waf_resource(
            "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny\"\n",
        ));
        let invalid = compile_plugins(&waf_resource("SecRule invalid-("));
        assert!(!invalid.is_activatable);
        assert!(invalid.retain_previous_on_invalid_update);
        assert!(invalid.preparse_errors.iter().any(|error| error.starts_with("waf: ")));

        let store = PluginStore::new();
        store.apply_delta(
            HashMap::from([("default/native-waf".to_string(), Arc::clone(&valid))]),
            HashMap::new(),
            &HashSet::new(),
        );
        let previous_runtime = store.get_plugin_runtime("default/native-waf").unwrap();
        store.apply_delta(
            HashMap::new(),
            HashMap::from([("default/native-waf".to_string(), Arc::clone(&invalid))]),
            &HashSet::new(),
        );

        let snapshot = store.scope();
        let retained = snapshot
            .http("default/native-waf")
            .expect("valid WAF generation remains published");
        assert!(retained.is_activatable);
        assert!(Arc::ptr_eq(
            first_filter(&retained.plugin_runtime),
            first_filter(&previous_runtime)
        ));
    }

    fn mirror_program_resource(program: &str, valid: bool, mirror: Option<serde_json::Value>) -> EdgionPlugins {
        let mut resource = waf_resource(if valid {
            "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny\"\n"
        } else {
            "SecRule invalid-("
        });
        let mut entries = Vec::new();
        if let Some(config) = mirror {
            entries.push(RequestFilterEntry::new(EdgionPlugin::RequestMirror(
                serde_json::from_value(config).expect("mirror fixture parses"),
            )));
        }
        if program == "dsl" {
            resource.spec.waf = None;
            entries.push(RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                name: "mirror-lkg".to_string(),
                source: Some(if valid { "let a = 1" } else { "invalid @@@" }.to_string()),
                ..Default::default()
            })));
        }
        resource.spec.request_plugins = Some(entries);
        resource
    }

    fn mirror_config(namespace: &str, denied: bool) -> serde_json::Value {
        let mut config = serde_json::json!({
            "backendRef": { "name": "mirror-lkg", "namespace": namespace, "port": 8080 }
        });
        if denied {
            config["backendRef"]["refDenied"] = serde_json::json!({
                "targetNamespace": namespace, "targetName": "mirror-lkg", "reason": "NoMatchingReferenceGrant"
            });
        }
        config
    }

    #[tokio::test]
    async fn policy_restriction_survives_invalid_program_publication() {
        for program in ["dsl", "waf"] {
            for full_snapshot in [false, true] {
                for case in [
                    "current-block",
                    "new-current-block",
                    "retained-only-block",
                    "bypass",
                    "removal",
                ] {
                    let entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(HTTPHeaderFilter {
                        set: None,
                        add: None,
                        remove: None,
                    }));
                    let mut old = mirror_program_resource(program, true, None);
                    if case != "new-current-block" {
                        old.spec.request_plugins.as_mut().unwrap().push(entry.clone());
                    }
                    let mut incoming = mirror_program_resource(program, false, None);
                    if case != "retained-only-block" {
                        incoming.spec.request_plugins.as_mut().unwrap().push(entry);
                    }
                    let key = old.key_name();
                    let store = PluginStore::new();
                    store.replace_all(HashMap::from([(key.clone(), compile_plugins(&old))]));
                    let in_flight = store.get_plugin_runtime(&key).unwrap();
                    let action = if case == "bypass" {
                        PluginPolicyAction::Bypass
                    } else {
                        PluginPolicyAction::Block { status: 451 }
                    };
                    store.set_test_policy(Some(policy_config("RequestHeaderModifier", action)));
                    let update = HashMap::from([(key.clone(), compile_plugins(&incoming))]);
                    if full_snapshot {
                        store.replace_all(update);
                    } else {
                        store.apply_delta(HashMap::new(), update, &HashSet::new());
                    }
                    if case == "removal" {
                        store.set_test_policy(None);
                    }
                    let scope = store.scope();
                    let published = scope.http(&key).unwrap();
                    assert_eq!(
                        published.is_activatable,
                        case != "removal",
                        "{program}/{full_snapshot}/{case}"
                    );
                    let names: Vec<_> = published
                        .plugin_runtime
                        .request_plugins_iter()
                        .map(|p| p.name())
                        .collect();
                    assert_eq!(
                        names.contains(&"RejectInvalidConfig"),
                        case == "removal",
                        "{case}: {names:?}"
                    );
                    assert_eq!(
                        in_flight
                            .request_plugins_iter()
                            .any(|p| p.name() == "RequestHeaderModifier"),
                        case != "new-current-block"
                    );
                    if case == "removal" {
                        assert_eq!(names[0], "RejectInvalidConfig");
                        assert!(!names.contains(&"PluginPolicyBlock"));
                        let mut session = MockPluginSession::new();
                        let mut log = PluginLog::new("RejectInvalidConfig");
                        assert!(matches!(
                            first_filter(&published.plugin_runtime)
                                .run_request(&mut session, &mut log)
                                .await,
                            PluginRunningResult::ErrResponse { status: 500, .. }
                        ));
                    } else if case == "bypass" {
                        assert!(names.contains(&"PluginPolicyBypass"));
                        assert!(!names.contains(&"RequestHeaderModifier"));
                    } else {
                        assert_eq!(names[0], "PluginPolicyBlock");
                        let mut session = MockPluginSession::new();
                        let mut log = PluginLog::new("PluginPolicyBlock");
                        assert!(matches!(
                            first_filter(&published.plugin_runtime)
                                .run_request(&mut session, &mut log)
                                .await,
                            PluginRunningResult::ErrResponse { status: 451, .. }
                        ));
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn mirror_authorization_updates_never_retain_cross_namespace_lkg() {
        for program in ["dsl", "waf"] {
            for full_snapshot in [false, true] {
                for incoming_mirror in [
                    Some(mirror_config("other", true)),
                    None,
                    Some(mirror_config("default", false)),
                    Some(mirror_config("retargeted", false)),
                ] {
                    let previous = compile_plugins(&mirror_program_resource(
                        program,
                        true,
                        Some(mirror_config("other", false)),
                    ));
                    let incoming = compile_plugins(&mirror_program_resource(program, false, incoming_mirror));
                    assert!(previous.is_activatable);
                    assert!(!incoming.is_activatable);
                    assert!(incoming.retain_previous_on_invalid_update);
                    let key = previous.spec.key_name();
                    let store = PluginStore::new();
                    store.replace_all(HashMap::from([(key.clone(), Arc::clone(&previous))]));
                    let in_flight = store.get_plugin_runtime(&key).expect("previous runtime is pinned");
                    let replacement = HashMap::from([(key.clone(), Arc::clone(&incoming))]);
                    if full_snapshot {
                        store.replace_all(replacement);
                    } else {
                        store.apply_delta(HashMap::new(), replacement, &HashSet::new());
                    }
                    let active = store.get_plugin_runtime(&key).expect("sealed replacement is published");
                    assert_eq!(first_filter(&active).name(), "RejectInvalidConfig");
                    assert!(in_flight.request_plugins_iter().any(|p| p.name() == "RequestMirror"));
                    assert!(!Arc::ptr_eq(&active, &in_flight));
                    assert_leading_reject(&incoming);
                    let reject = active.request_plugins_iter().next().expect("reject precedes mirror");
                    let mut session = MockPluginSession::new();
                    let mut log = PluginLog::new(reject.name());
                    assert!(matches!(
                        reject.run_request(&mut session, &mut log).await,
                        PluginRunningResult::ErrResponse { status: 500, .. }
                    ));

                    let recovered = compile_plugins(&mirror_program_resource(
                        program,
                        true,
                        Some(mirror_config("other", true)),
                    ));
                    assert!(recovered.is_activatable);
                    store.apply_delta(
                        HashMap::new(),
                        HashMap::from([(key.clone(), Arc::clone(&recovered))]),
                        &HashSet::new(),
                    );
                    let recovered_runtime = store.get_plugin_runtime(&key).expect("valid update recovers");
                    assert!(!Arc::ptr_eq(&active, &recovered_runtime));
                    let mirror = recovered_runtime
                        .request_plugins_iter()
                        .find(|filter| filter.name() == "RequestMirror")
                        .expect("valid denied mirror remains executable");
                    let mut session = MockPluginSession::new();
                    let mut log = PluginLog::new(mirror.name());
                    assert!(matches!(
                        mirror.run_request(&mut session, &mut log).await,
                        PluginRunningResult::GoodNext
                    ));
                }
            }
        }
    }

    #[test]
    fn mirror_lkg_boundary_checks_incoming_and_preserves_inert_or_local_mirrors() {
        for program in ["dsl", "waf"] {
            for full_snapshot in [false, true] {
                for case in ["added", "local", "implicit", "disabled", "bypassed", "absent"] {
                    let config = match case {
                        "absent" | "added" => None,
                        "implicit" => Some(serde_json::json!({"backendRef": {"name": "mirror-lkg", "port": 8080}})),
                        "local" => Some(mirror_config("default", false)),
                        _ => Some(mirror_config("other", false)),
                    };
                    let mut old_resource = mirror_program_resource(program, true, config.clone());
                    let mut new_resource = mirror_program_resource(
                        program,
                        false,
                        if case == "added" {
                            Some(mirror_config("other", true))
                        } else {
                            config
                        },
                    );
                    for resource in [&mut old_resource, &mut new_resource] {
                        if case == "implicit" {
                            resource.metadata.namespace = None;
                        }
                        if let Some(entry) = resource
                            .spec
                            .request_plugins
                            .as_mut()
                            .and_then(|entries| entries.first_mut())
                        {
                            if case == "disabled" {
                                entry.enable = false;
                            }
                        }
                    }
                    let action = (case == "bypassed").then_some(PluginPolicyAction::Bypass);
                    let previous = compile_with_policy(&old_resource, "RequestMirror", action);
                    let incoming = compile_with_policy(&new_resource, "RequestMirror", action);
                    assert!(previous.is_activatable, "{case}");
                    assert!(!incoming.is_activatable, "{case}");
                    assert!(incoming.retain_previous_on_invalid_update, "{case}");
                    // Global references always use a namespace; the source still has
                    // no namespace in the implicit-local mirror case.
                    let key = "default/native-waf".to_string();
                    let store = PluginStore::new();
                    store.set_test_policy(action.map(|action| policy_config("RequestMirror", action)));
                    store.replace_all(HashMap::from([(key.clone(), compile_plugins(&old_resource))]));
                    let previous_runtime = store.get_plugin_runtime(&key).unwrap();
                    let replacement = HashMap::from([(key.clone(), compile_plugins(&new_resource))]);
                    if full_snapshot {
                        store.replace_all(replacement);
                    } else {
                        store.apply_delta(HashMap::new(), replacement, &HashSet::new());
                    }
                    let active = store.get_plugin_runtime(&key).expect("runtime remains published");
                    if case == "added" {
                        assert_eq!(first_filter(&active).name(), "RejectInvalidConfig");
                    } else {
                        // Compare an executable entry, excluding freshly assembled policy gates.
                        let active_entry = active
                            .request_plugins_iter()
                            .find(|p| !p.name().starts_with("PluginPolicy"))
                            .unwrap();
                        let previous_entry = previous_runtime
                            .request_plugins_iter()
                            .find(|p| !p.name().starts_with("PluginPolicy"))
                            .unwrap();
                        assert!(
                            Arc::ptr_eq(active_entry, previous_entry),
                            "{program}/{full_snapshot}/{case}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_waf_same_key_add_replaces_last_known_good_and_fails_closed() {
        let key = "default/native-waf".to_string();
        let valid = compile_plugins(&waf_resource(
            "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny\"\n",
        ));
        let invalid = compile_plugins(&waf_resource("SecRule invalid-("));
        let store = PluginStore::new();

        store.apply_delta(
            HashMap::from([(key.clone(), Arc::clone(&valid))]),
            HashMap::new(),
            &HashSet::new(),
        );
        store.apply_delta(
            HashMap::from([(key.clone(), Arc::clone(&invalid))]),
            HashMap::new(),
            &HashSet::new(),
        );

        let snapshot = store.scope();
        let replacement = snapshot.http(&key).expect("invalid add remains stored");
        assert_eq!(replacement.preparse_errors, invalid.preparse_errors);
        assert_leading_reject(&replacement);
        assert!(!replacement.is_activatable);
    }

    #[test]
    fn waf_store_publishes_atomically_and_pins_in_flight_generations() {
        let key = "default/native-waf".to_string();
        let first = compile_plugins(&waf_resource(
            "SecRule REQUEST_URI \"@contains /first\" \"id:1,phase:1,deny\"\n",
        ));
        let second = compile_plugins(&waf_resource(
            "SecRule REQUEST_URI \"@contains /second\" \"id:2,phase:1,deny\"\n",
        ));
        let invalid = compile_plugins(&waf_resource("SecRule invalid-("));
        let recovered = compile_plugins(&waf_resource(
            "SecRule REQUEST_URI \"@contains /third\" \"id:3,phase:1,deny\"\n",
        ));
        let store = PluginStore::new();

        store.replace_all(HashMap::from([(key.clone(), Arc::clone(&first))]));
        let in_flight_first = store.get_plugin_runtime(&key).expect("first generation is published");

        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::clone(&second))]),
            &HashSet::new(),
        );
        let in_flight_second = store.get_plugin_runtime(&key).expect("second generation is published");
        assert_eq!(in_flight_first.request_plugins_count(), 1);
        assert_eq!(in_flight_second.request_plugins_count(), 1);
        assert!(!Arc::ptr_eq(
            first_filter(&in_flight_first),
            first_filter(&in_flight_second)
        ));
        assert!(!Arc::ptr_eq(&in_flight_first, &in_flight_second));

        store.replace_all(HashMap::from([(key.clone(), invalid)]));
        let retained = store
            .get_plugin_runtime(&key)
            .expect("invalid full snapshot retains the second generation");
        assert!(Arc::ptr_eq(first_filter(&retained), first_filter(&in_flight_second)));

        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::clone(&recovered))]),
            &HashSet::new(),
        );
        let active_recovered = store
            .get_plugin_runtime(&key)
            .expect("recovered generation is published");
        assert!(!Arc::ptr_eq(
            first_filter(&active_recovered),
            first_filter(&in_flight_second)
        ));

        store.apply_delta(HashMap::new(), HashMap::new(), &HashSet::from([key.clone()]));
        assert!(store.get_plugin_runtime(&key).is_none());
        assert_eq!(in_flight_first.request_plugins_count(), 1);
        assert_eq!(in_flight_second.request_plugins_count(), 1);
        assert_eq!(active_recovered.request_plugins_count(), 1);
    }

    #[test]
    fn waf_reload_storm_never_publishes_an_invalid_or_partial_runtime() {
        let key = "default/native-waf".to_string();
        let first = compile_plugins(&waf_resource(
            "SecRule REQUEST_URI \"@contains /first\" \"id:1,phase:1,deny\"\n",
        ));
        let second = compile_plugins(&waf_resource(
            "SecRule REQUEST_URI \"@contains /second\" \"id:2,phase:1,deny\"\n",
        ));
        let invalid = compile_plugins(&waf_resource("SecRule invalid-("));
        let store = Arc::new(PluginStore::new());
        store.replace_all(HashMap::from([(key.clone(), Arc::clone(&first))]));

        std::thread::scope(|scope| {
            for _ in 0..4 {
                let store = Arc::clone(&store);
                let key = key.clone();
                scope.spawn(move || {
                    for _ in 0..10_000 {
                        let published = store.scope().http(&key).expect("reload never removes the active WAF");
                        let runtime = &published.plugin_runtime;
                        let rules = &published.spec.spec.waf.as_ref().unwrap().rules;
                        assert!(published.is_activatable);
                        assert!(rules.contains("/first") || rules.contains("/second"));
                        assert_eq!(runtime.request_plugins_count(), 1);
                        assert_eq!(first_filter(runtime).name(), "Waf");
                        assert_eq!(runtime.upstream_response_plugins_count(), 1);
                        assert_eq!(runtime.upstream_response_async_plugins_count(), 1);
                    }
                });
            }

            let store = Arc::clone(&store);
            let key = key.clone();
            let first = Arc::clone(&first);
            let second = Arc::clone(&second);
            let invalid = Arc::clone(&invalid);
            scope.spawn(move || {
                for iteration in 0..2_000 {
                    let generation = match iteration % 3 {
                        0 => Arc::clone(&first),
                        1 => Arc::clone(&invalid),
                        _ => Arc::clone(&second),
                    };
                    store.apply_delta(
                        HashMap::new(),
                        HashMap::from([(key.clone(), generation)]),
                        &HashSet::new(),
                    );
                }
            });
        });

        let active = store
            .get_plugin_runtime(&key)
            .expect("a complete generation remains after the storm");
        assert_eq!(active.request_plugins_count(), 1);
        assert_eq!(first_filter(&active).name(), "Waf");
    }

    /// A `ResponseHeaderModifier` plugin placed under `requestPlugins` is a
    /// response-stage plugin mis-configured into the request stage. `compile_plugins`
    /// must surface the mismatch in `preparse_errors` so the Controller writes it
    /// into `status.conditions` — silently dropping it would leave `Accepted: True`
    /// even though the plugin never ran (auth-bypass class bug).
    #[test]
    fn compile_plugins_flags_stage_mismatch() {
        // ResponseHeaderModifier → request stage (wrong stage)
        let request_plugins = vec![RequestFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
            HTTPHeaderFilter {
                set: None,
                add: None,
                remove: Some(vec!["X-Remove".into()]),
            },
        ))];
        // RequestHeaderModifier → upstream response filter stage (wrong stage)
        let response_filter_plugins = vec![UpstreamResponseFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
            HTTPHeaderFilter {
                set: None,
                add: None,
                remove: Some(vec!["X-Remove".into()]),
            },
        ))];

        let ep = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                request_plugins: Some(request_plugins),
                upstream_response_filter_plugins: Some(response_filter_plugins),
                ..Default::default()
            },
            status: None,
        };

        let compiled = compile_plugins(&ep);
        let errors = &compiled.preparse_errors;
        assert!(
            errors
                .iter()
                .any(|e| e.contains("requestPlugins[0]") && e.contains("not valid for request stage")),
            "expected request-stage mismatch error, got: {:?}",
            errors
        );
        assert!(
            errors.iter().any(|e| e.contains("upstreamResponseFilterPlugins[0]")
                && e.contains("not valid for upstream response filter stage")),
            "expected upstream-response-filter-stage mismatch error, got: {:?}",
            errors
        );
    }

    /// Regression test: valid entries must NOT produce stage-mismatch errors.
    #[test]
    fn compile_plugins_no_false_positive_on_valid_stages() {
        let request_plugins = vec![RequestFilterEntry::new(header_modifier())];
        let response_filter_plugins = vec![UpstreamResponseFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
            HTTPHeaderFilter {
                set: None,
                add: None,
                remove: Some(vec!["X-Remove".into()]),
            },
        ))];

        let ep = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                request_plugins: Some(request_plugins),
                upstream_response_filter_plugins: Some(response_filter_plugins),
                ..Default::default()
            },
            status: None,
        };

        let compiled = compile_plugins(&ep);
        assert!(
            !compiled.preparse_errors.iter().any(|e| e.contains("not valid for")),
            "valid stage placement should not produce mismatch errors, got: {:?}",
            compiled.preparse_errors
        );
    }

    #[test]
    fn unresolved_entry_condition_seals_resource_fail_closed() {
        let mut entry = RequestFilterEntry::new(header_modifier());
        entry.conditions = Some(
            serde_json::from_value(serde_json::json!({
                "run": { "allOf": [{
                        "type": "remoteIpMatch",
                        "ipRefs": [{ "name": "missing" }],
                        "negate": true
                }] }
            }))
            .unwrap(),
        );
        let ep = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![entry]),
                ..Default::default()
            },
            status: None,
        };

        let compiled = compile_plugins(&ep);
        assert!(!compiled.is_activatable);
        assert!(compiled
            .preparse_errors
            .iter()
            .any(|error| error.contains("conditions contain unresolved references")));
        assert_eq!(compiled.plugin_runtime.request_plugins_count(), 2);
    }

    /// C1 regression test: a hard-blocklisted `accessLogExtern` declaration must
    /// be dropped from the *stored* spec by `compile_plugins`, not merely reported.
    /// Without the `retain` in `compile_plugins`, `authorization` would survive
    /// into `compiled.spec` and `apply_declarations` would log it in clear
    /// whenever `access_log_sensitive.mode` is `off` or `wrap`.
    #[test]
    fn compile_plugins_drops_blocked_access_log_extern_fields() {
        let ep = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                access_log_extern: Some(vec![
                    AccessLogExternField {
                        key: "leak".to_string(),
                        from: LogExtSource::Header,
                        name: "authorization".to_string(),
                    },
                    AccessLogExternField {
                        key: "tenant".to_string(),
                        from: LogExtSource::Ctx,
                        name: "tenant_id".to_string(),
                    },
                ]),
                ..Default::default()
            },
            status: None,
        };

        let compiled = compile_plugins(&ep);

        let stored = compiled.spec.access_log_extern_fields();
        assert_eq!(
            stored.len(),
            1,
            "the blocked field must not survive into the stored spec"
        );
        assert_eq!(stored[0].key, "tenant", "the valid field must be retained");

        assert!(
            compiled
                .preparse_errors
                .iter()
                .any(|e| e.contains("accessLogExtern") && e.contains("authorization") && e.contains("blocked")),
            "expected a preparse error naming the dropped 'authorization' field, got: {:?}",
            compiled.preparse_errors
        );
    }

    /// Drift guard between the two independent validation implementations.
    ///
    /// Since the controller no longer depends on `edgion-gateway`, it collects
    /// the EdgionPlugins K8s-status preparse errors via the schema-layer
    /// `EdgionPlugins::validate_plugins` (in `edgion-resources`), while the data
    /// plane still produces them as a side effect of `compile_plugins` building
    /// the runtime. These are SEPARATE code paths that must stay in lockstep —
    /// if `validate_plugins` ever drops an error that `compile_plugins` still
    /// reports, the controller would mark a genuinely-broken EdgionPlugins
    /// `Accepted: True` (e.g. an auth plugin in the wrong stage that never runs).
    ///
    /// Compared as sorted sets: exact message text and ordering parity are
    /// covered by the unit tests in
    /// `edgion_resources::resources::edgion_plugins::validate`; this guard's job
    /// is the stronger safety property that neither path has an error the other
    /// lacks. DSL entries are included: since the edgion-dsl extraction, both
    /// paths call the same `DslConfig::validation_error()` pipeline, so they
    /// must match exactly.
    ///
    /// **Deliberate exclusion**: `validate_plugins`'s `accessLogExtern` Pass 3
    /// also enforces a `LOG_LABEL_MAX_COUNT` cap and a duplicate-`key` check
    /// (`edgion-resources/src/resources/edgion_plugins/validate.rs`), which
    /// `compile_plugins`'s `retain()` does not mirror and which this guard does
    /// not probe. That is intentional, not a gap to close: the data plane is
    /// already structurally immune to both without needing to reject anything —
    /// `LogExt::insert` caps stored fields at 16 regardless of how many were
    /// declared, and a duplicate key resolves last-write-wins in place. So
    /// `validate_plugins` over-reports relative to `compile_plugins` on these
    /// two rules specifically (controller status flags something the gateway
    /// was never going to mis-render), which is the safe direction for the
    /// asymmetry to run. Do not add these two rules to `compile_plugins` to
    /// "fix" this guard — there is no safety gain, only duplicated logic.
    #[test]
    fn validate_plugins_matches_compile_plugins_error_set() {
        fn sorted(mut v: Vec<String>) -> Vec<String> {
            v.sort();
            v
        }
        fn hf() -> HTTPHeaderFilter {
            HTTPHeaderFilter {
                set: None,
                add: None,
                remove: None,
            }
        }
        // RequestHeaderModifier: request-stage-only. ResponseHeaderModifier:
        // upstream-response-filter-stage-only. Neither has a `validation_error`,
        // so any error comes purely from alias/condition/stage checks.
        fn rhm() -> EdgionPlugin {
            EdgionPlugin::RequestHeaderModifier(hf())
        }
        fn respm() -> EdgionPlugin {
            EdgionPlugin::ResponseHeaderModifier(hf())
        }
        fn dsl(source: &str) -> EdgionPlugin {
            EdgionPlugin::Dsl(DslConfig {
                name: "d".to_string(),
                source: Some(source.to_string()),
                ..Default::default()
            })
        }
        fn request_body_buffer() -> EdgionPlugin {
            EdgionPlugin::RequestBodyBuffer(Default::default())
        }
        fn json_schema() -> EdgionPlugin {
            use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;
            EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
                schema: serde_json::json!({"type": "object"}),
            })
        }
        fn json_schema_entry_with_body(max_body_size: Option<&str>, with_conditions: bool) -> RequestFilterEntry {
            use edgion_resources::resources::edgion_plugins::BodyRequirement;
            let mut entry = RequestFilterEntry::new(json_schema());
            entry.body = Some(BodyRequirement {
                max_body_size: max_body_size.map(str::to_string),
                conditions: with_conditions.then(Default::default),
                ..Default::default()
            });
            entry
        }
        fn invalid_json_schema_with_invalid_body() -> RequestFilterEntry {
            use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;

            let mut entry = json_schema_entry_with_body(Some("notasize"), false);
            entry.plugin = EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
                schema: serde_json::json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
            });
            entry
        }
        fn invalid_proxy_rewrite_with_invalid_body() -> RequestFilterEntry {
            use edgion_resources::resources::edgion_plugins::{JsonBodyRewrite, ProxyRewriteConfig, RegexUri};

            let mut entry = RequestFilterEntry::new(EdgionPlugin::ProxyRewrite(ProxyRewriteConfig {
                regex_uri: Some(RegexUri {
                    pattern: "(".to_string(),
                    replacement: "/invalid".to_string(),
                }),
                json_body: Some(JsonBodyRewrite::default()),
                ..Default::default()
            }));
            entry.body = Some(edgion_resources::resources::edgion_plugins::BodyRequirement {
                max_body_size: Some("notasize".to_string()),
                ..Default::default()
            });
            entry
        }
        fn aliased(mut e: RequestFilterEntry, alias: &str) -> RequestFilterEntry {
            e.alias = Some(alias.to_string());
            e
        }
        // Disabled entry carrying a malformed regex condition. Both
        // `compile_plugin_conditions` (runtime) and `validate_plugins` Pass 1
        // (schema) must skip it, so the error set stays empty on both sides.
        fn disabled_with_bad_condition() -> RequestFilterEntry {
            let mut e = RequestFilterEntry::with_enable(rhm(), false);
            e.conditions = Some(
                serde_json::from_value(serde_json::json!({
                    "run": { "allOf": [{
                            "type": "keyMatch",
                            "key": { "type": "header", "name": "X-Test" },
                            "regex": ["("]
                    }] }
                }))
                .expect("build malformed conditions"),
            );
            e
        }
        fn with_bad_body(mut e: RequestFilterEntry) -> RequestFilterEntry {
            e.body = Some(edgion_resources::resources::edgion_plugins::BodyRequirement {
                max_body_size: Some("notasize".to_string()),
                ..Default::default()
            });
            e
        }
        // A body-consuming plugin (HMAC with validateRequestBody) whose operator
        // `body.maxBodySize` is malformed — exercises the maxBodySize validation parity,
        // distinct from the "plugin does not consume a body" path.
        fn hmac_body_consumer() -> EdgionPlugin {
            use edgion_resources::resources::edgion_plugins::HmacAuthConfig;
            EdgionPlugin::HmacAuth(HmacAuthConfig {
                match_config: edgion_resources::resources::edgion_plugins::HmacMatchConfig {
                    validate_request_body: true,
                    ..Default::default()
                },
                anonymous: Some("anon".to_string()),
                ..Default::default()
            })
        }
        // A bad regex inside the secondary body-gate conditions — exercises the
        // `body conditions regex` compilation parity (F3).
        fn with_bad_body_regex(mut e: RequestFilterEntry) -> RequestFilterEntry {
            use edgion_resources::resources::edgion_plugins::conditions::{
                Condition, ConditionSet, KeyMatchCondition, PluginConditions,
            };
            e.body = Some(edgion_resources::resources::edgion_plugins::BodyRequirement {
                conditions: Some(PluginConditions {
                    skip: None,
                    run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
                        key: edgion_resources::common::KeyGet::Header { name: "x".to_string() },
                        core: edgion_resources::resources::edgion_plugins::conditions::MatchCore {
                            regex: Some(vec!["[".to_string()]),
                            ..Default::default()
                        },
                        negate: false,
                        descriptor: Box::from(""),
                    })])),
                }),
                ..Default::default()
            });
            e
        }
        fn disabled_invalid_json_schema() -> RequestFilterEntry {
            use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;

            let mut entry = json_schema_entry_with_body(Some("notasize"), true);
            entry.plugin = EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
                schema: serde_json::json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
            });
            entry.alias = Some("invalid alias".to_string());
            entry.conditions = disabled_with_bad_condition().conditions;
            entry.enable = false;
            entry
        }
        fn mk(
            request: Option<Vec<RequestFilterEntry>>,
            urf: Option<Vec<UpstreamResponseFilterEntry>>,
        ) -> EdgionPlugins {
            EdgionPlugins {
                metadata: Default::default(),
                spec: EdgionPluginsSpec {
                    request_plugins: request,
                    upstream_response_filter_plugins: urf,
                    ..Default::default()
                },
                status: None,
            }
        }
        // C1 case: a hard-blocklisted `accessLogExtern` field. Before the C1 fix,
        // `compile_plugins` never looked at `access_log_extern` at all, so this
        // case failed with `from_runtime` empty and `from_schema` carrying the
        // blocked-name error — exactly the drift class this guard exists to catch.
        fn ep_with_blocked_access_log_extern() -> EdgionPlugins {
            EdgionPlugins {
                metadata: Default::default(),
                spec: EdgionPluginsSpec {
                    access_log_extern: Some(vec![AccessLogExternField {
                        key: "leak".to_string(),
                        from: LogExtSource::Header,
                        name: "authorization".to_string(),
                    }]),
                    ..Default::default()
                },
                status: None,
            }
        }

        let ai_guard = || {
            let config: edgion_resources::resources::edgion_plugins::AiGuardConfig = serde_yaml::from_str(
                "rules:\n  - name: r\n    match: { type: regex, pattern: \"x\" }\n    action: reject\n",
            )
            .expect("configuration parses");
            RequestFilterEntry::new(EdgionPlugin::AiGuard(config))
        };
        let ai_proxy = || {
            let config: edgion_resources::resources::edgion_plugins::AiProxyConfig =
                serde_yaml::from_str("{}").expect("configuration parses");
            RequestFilterEntry::new(EdgionPlugin::AiProxy(config))
        };

        let cases: Vec<(&str, EdgionPlugins)> = vec![
            ("valid_request", mk(Some(vec![RequestFilterEntry::new(rhm())]), None)),
            (
                // A guard ordered after AiProxy can never affect the provider
                // body. Both paths must report the identical message; separate
                // publication tests verify the unconditional fail-closed gate.
                "ai_guard_after_ai_proxy",
                mk(Some(vec![ai_proxy(), ai_guard()]), None),
            ),
            (
                // Two entries of one stage array claiming the same alias → the
                // second is rejected. The alias is part of the entry identity
                // that namespaces external state (RateLimitRedis buckets).
                "duplicate_alias_in_stage",
                mk(
                    Some(vec![
                        aliased(RequestFilterEntry::new(rhm()), "dup"),
                        aliased(RequestFilterEntry::new(rhm()), "dup"),
                    ]),
                    None,
                ),
            ),
            (
                // A bounded-structured-body error owns the entry in BOTH layers, so
                // the entry never claims its alias and the later duplicate is NOT
                // reported. Pins the skip-set parity the alias accumulator depends on.
                //
                // The first entry must have a VALID plugin config (a config error
                // would short-circuit both layers earlier, before the bounded-body
                // skip, making this case vacuous): valid `{"type":"object"}` schema
                // with a malformed `body.maxBodySize`.
                "duplicate_alias_behind_invalid_bounded_body",
                mk(
                    Some(vec![
                        aliased(json_schema_entry_with_body(Some("notasize"), false), "dup"),
                        aliased(RequestFilterEntry::new(rhm()), "dup"),
                    ]),
                    None,
                ),
            ),
            (
                // The same alias in two different stage arrays is fine: the
                // accumulator is per stage.
                "same_alias_across_stages",
                mk(
                    Some(vec![aliased(RequestFilterEntry::new(rhm()), "shared")]),
                    Some(vec![{
                        let mut e = UpstreamResponseFilterEntry::new(respm());
                        e.alias = Some("shared".to_string());
                        e
                    }]),
                ),
            ),
            (
                // `body:` on a plugin that does not consume a body → rejected.
                "body_on_non_consuming_plugin",
                mk(Some(vec![with_bad_body(RequestFilterEntry::new(rhm()))]), None),
            ),
            (
                // `body.maxBodySize` malformed on a body-consuming plugin → maxBodySize error.
                "bad_body_max_body_size_on_consumer",
                mk(
                    Some(vec![with_bad_body(RequestFilterEntry::new(hmac_body_consumer()))]),
                    None,
                ),
            ),
            (
                // Bad regex in the secondary body-gate conditions → compile error.
                "bad_body_condition_regex",
                mk(Some(vec![with_bad_body_regex(RequestFilterEntry::new(rhm()))]), None),
            ),
            (
                // Disabled entry with a bad body-condition regex → inert, both
                // sides skip it (empty error set), same as disabled_with_bad_condition.
                "disabled_with_bad_body_regex",
                mk(
                    Some(vec![with_bad_body_regex(RequestFilterEntry::with_enable(rhm(), false))]),
                    None,
                ),
            ),
            (
                "valid_urf",
                mk(None, Some(vec![UpstreamResponseFilterEntry::new(respm())])),
            ),
            (
                "wrong_stage_request",
                mk(Some(vec![RequestFilterEntry::new(respm())]), None),
            ),
            (
                "wrong_stage_urf",
                mk(None, Some(vec![UpstreamResponseFilterEntry::new(rhm())])),
            ),
            (
                "bad_alias_valid_stage",
                mk(Some(vec![aliased(RequestFilterEntry::new(rhm()), "bad alias")]), None),
            ),
            (
                "bad_alias_wrong_stage",
                mk(Some(vec![aliased(RequestFilterEntry::new(respm()), "bad alias")]), None),
            ),
            (
                "disabled_wrong_stage",
                mk(Some(vec![RequestFilterEntry::with_enable(respm(), false)]), None),
            ),
            (
                "disabled_bad_condition",
                mk(Some(vec![disabled_with_bad_condition()]), None),
            ),
            (
                "bad_entry_time_range_condition",
                entry_condition_resource(malformed_time_range_conditions()),
            ),
            (
                "bad_entry_regex_condition",
                entry_condition_resource(bad_regex_conditions()),
            ),
            (
                "bad_body_time_range_condition",
                body_condition_resource(malformed_time_range_conditions()),
            ),
            (
                "bad_body_regex_condition",
                body_condition_resource(bad_regex_conditions()),
            ),
            (
                "bad_dye_time_range_condition",
                dye_condition_resource(malformed_time_range_conditions()),
            ),
            (
                "bad_dye_regex_condition",
                dye_condition_resource(bad_regex_conditions()),
            ),
            (
                "mixed_multi_stage",
                mk(
                    Some(vec![RequestFilterEntry::new(rhm()), RequestFilterEntry::new(respm())]),
                    Some(vec![
                        UpstreamResponseFilterEntry::new(respm()),
                        UpstreamResponseFilterEntry::new(rhm()),
                    ]),
                ),
            ),
            (
                "dsl_valid",
                mk(Some(vec![RequestFilterEntry::new(dsl("let a = 1"))]), None),
            ),
            (
                "request_body_buffer_valid",
                mk(Some(vec![RequestFilterEntry::new(request_body_buffer())]), None),
            ),
            (
                "request_body_buffer_wrong_stage",
                mk(
                    None,
                    Some(vec![UpstreamResponseFilterEntry::new(request_body_buffer())]),
                ),
            ),
            (
                "json_schema_invalid_body_limit",
                mk(Some(vec![json_schema_entry_with_body(Some("notasize"), false)]), None),
            ),
            (
                "json_schema_zero_body_limit",
                mk(Some(vec![json_schema_entry_with_body(Some("0"), false)]), None),
            ),
            (
                "json_schema_body_limit_above_absolute_max",
                mk(Some(vec![json_schema_entry_with_body(Some("1048577"), false)]), None),
            ),
            (
                "json_schema_body_conditions",
                mk(Some(vec![json_schema_entry_with_body(None, true)]), None),
            ),
            (
                "json_schema_invalid_config_and_body",
                mk(Some(vec![invalid_json_schema_with_invalid_body()]), None),
            ),
            (
                "proxy_rewrite_invalid_config_and_body",
                mk(Some(vec![invalid_proxy_rewrite_with_invalid_body()]), None),
            ),
            (
                "json_schema_two_enabled",
                mk(
                    Some(vec![
                        RequestFilterEntry::new(json_schema()),
                        RequestFilterEntry::new(json_schema()),
                    ]),
                    None,
                ),
            ),
            (
                "json_schema_enabled_and_disabled",
                mk(
                    Some(vec![
                        RequestFilterEntry::new(json_schema()),
                        RequestFilterEntry::with_enable(json_schema(), false),
                    ]),
                    None,
                ),
            ),
            (
                "disabled_invalid_json_schema_is_inert",
                mk(
                    Some(vec![
                        disabled_invalid_json_schema(),
                        RequestFilterEntry::new(json_schema()),
                    ]),
                    None,
                ),
            ),
            (
                "dsl_bad_syntax",
                mk(
                    Some(vec![RequestFilterEntry::new(dsl("this is not valid dsl @@@"))]),
                    None,
                ),
            ),
            (
                "dsl_unknown_redis_link",
                mk(
                    Some(vec![RequestFilterEntry::new(dsl(
                        "let r = redis.get(\"mylink\", \"k\")",
                    ))]),
                    None,
                ),
            ),
            ("access_log_extern_blocked", ep_with_blocked_access_log_extern()),
            ("waf_blank_rules", waf_resource(" \n\t")),
            ("waf_bad_syntax", waf_resource("SecRule invalid-(")),
        ];

        for (name, ep) in cases {
            let from_runtime = sorted(compile_plugins(&ep).preparse_errors.clone());
            let from_schema = sorted(ep.validate_plugins());
            assert_eq!(
                from_runtime, from_schema,
                "preparse error-set drift between compile_plugins (runtime) and validate_plugins (schema) for case `{name}`"
            );
        }
    }
}
