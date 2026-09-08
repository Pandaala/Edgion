//! Global store for EdgionPlugins resources.
//!
//! The store holds **compiled** plugins (`Arc<CompiledPlugins>`): the immutable
//! serializable `EdgionPlugins` schema paired with the gateway-built
//! `PluginRuntime`. The runtime is built once per (namespace/name) by
//! `compile_plugins` and Arc-shared via the store — never duplicated, because
//! in-memory `RateLimit` owns per-instance CMS state that a second build would
//! split. This side-table indirection (dynamic store key, not a compiled-in
//! reference) is what supports hot reload.

use arc_swap::ArcSwap;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use crate::plugins::runtime::conditions::PluginConditionsExt;
use crate::plugins::runtime::{InvalidUpdatePolicy, PluginRuntime};
use edgion_resources::resource::meta::ResourceMeta;
use edgion_resources::resources::EdgionPlugins;

static GLOBAL_PLUGIN_STORE: LazyLock<Arc<PluginStore>> = LazyLock::new(|| Arc::new(PluginStore::new()));

pub fn get_global_plugin_store() -> Arc<PluginStore> {
    GLOBAL_PLUGIN_STORE.clone()
}

/// Borrow the global store without paying the `Arc` clone (an atomic RMW pair).
/// Prefer this over [`get_global_plugin_store`] on the per-request hot path — e.g.
/// `log_ext::collect`, which reads `has_access_log_extern()` on every request and
/// must not pay a clone before its own fast-path check.
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
    /// The compiled plugin runtime, built once per (ns/name) and Arc-shared.
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
    /// DSL source/bytecode is a compiled program, so a rejected program update
    /// follows last-known-good publication. Other plugin configuration errors
    /// keep the established fail-closed replacement behavior.
    pub retain_previous_on_invalid_update: bool,
}

/// Build the compiled runtime for an EdgionPlugins resource.
///
/// Mirrors the former `EdgionPlugins::preparse()`: compile regex conditions on
/// every entry, build the `PluginRuntime` across all four stages, then
/// `seal_fail_closed` so any invalid plugin makes every request through this
/// resource fail closed with HTTP 500.
///
/// Built **once per (ns/name)** and Arc-shared via the store; never per request
/// or per referencing route.
pub fn compile_plugins(ep: &EdgionPlugins) -> Arc<CompiledPlugins> {
    let mut spec = ep.clone();
    let mut errors = Vec::new();
    let mut runtime = PluginRuntime::new();
    let namespace = spec.metadata.namespace.clone().unwrap_or_else(|| "default".to_string());
    // Identity of the resource as the store keys it, so a plugin-entry scope
    // derived from it is unique exactly when the store entry is: re-deriving
    // `ns/name` here instead would normalize a missing namespace to "default"
    // and alias two distinct store entries.
    let resource_key = spec.key_name();

    if let Some(waf) = spec.spec.waf.as_ref() {
        if let Err(error) = runtime.add_waf(waf) {
            errors.push(format!("waf: {error}"));
        }
    }

    // Compile regex conditions on all plugin entries once, before building the
    // runtime. This avoids per-request Regex::new() in the fallback path of
    // condition evaluators. add_from_* clones the (now-compiled) conditions into
    // the runtime's ConditionalFilters.
    compile_plugin_conditions(&mut spec, &mut errors, &mut runtime);
    // Mirror the controller's cross-entry ordering rule into the data plane,
    // from the same shared helper and with the same message, so
    // `validate_plugins_matches_compile_plugins_error_set` stays exact. This is
    // not cosmetic: a misordered guard means prompts reach the provider
    // unguarded, so the resource must also fail closed here rather than only
    // being flagged in status.
    if let Some(entries) = spec.spec.request_plugins.as_ref() {
        edgion_resources::resources::edgion_plugins::validate::collect_ai_guard_ordering_errors(entries, &mut errors);
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
        errors.extend(runtime.add_from_request_filters(request_plugins, &namespace, &resource_key));
    }
    if let Some(filter_plugins) = &spec.spec.upstream_response_filter_plugins {
        errors.extend(runtime.add_from_upstream_response_filters(filter_plugins, &namespace));
    }
    if let Some(body_filter_plugins) = &spec.spec.upstream_response_body_filter_plugins {
        errors.extend(runtime.add_from_upstream_response_body_filters(body_filter_plugins, &namespace));
    }
    if let Some(response_plugins) = &spec.spec.upstream_response_plugins {
        errors.extend(runtime.add_from_upstream_responses(response_plugins, &namespace));
    }

    // Resource-level dye "set clobber" check: a failure rule's response
    // header must not equal (case-insensitively) any success rule's response
    // header across ANY entry in this resource — success renders with Set
    // semantics and would silently wipe an appended failure value queued
    // earlier by a different entry. Runs once over all four stage arrays
    // chained, mirroring the schema-layer `EdgionPlugins::validate_plugins`
    // check (only active, non-policy-stamped entries participate, matching
    // `add_from_*` above).
    let all_dye_suites = spec
        .spec
        .request_plugins
        .iter()
        .flatten()
        .filter(|e| e.is_enabled() && e.policy_action.is_none())
        .map(|e| e.dye.as_ref())
        .chain(
            spec.spec
                .upstream_response_filter_plugins
                .iter()
                .flatten()
                .filter(|e| e.is_enabled() && e.policy_action.is_none())
                .map(|e| e.dye.as_ref()),
        )
        .chain(
            spec.spec
                .upstream_response_body_filter_plugins
                .iter()
                .flatten()
                .filter(|e| e.is_enabled() && e.policy_action.is_none())
                .map(|e| e.dye.as_ref()),
        )
        .chain(
            spec.spec
                .upstream_response_plugins
                .iter()
                .flatten()
                .filter(|e| e.is_enabled() && e.policy_action.is_none())
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
                    // Disabled and PluginPolicy-stamped entries are never built
                    // or run, so dormant conditions must not produce a preparse
                    // error. This stays in lockstep with schema validation.
                    if !entry.is_enabled() || entry.policy_action.is_some() {
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
                        if let Err(errs) = conds.validate() {
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
                    if !entry.is_enabled() || entry.policy_action.is_some() {
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
                    if !entry.is_enabled() || entry.policy_action.is_some() {
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

/// Type alias for the compiled-plugin map (key: namespace/name)
type PluginMap = HashMap<String, Arc<CompiledPlugins>>;

/// One immutable plugin-store generation.
///
/// The access-log hint is derived from `plugins`, so both values must be
/// published together. A reader that loads this snapshot once can never
/// observe the hint from one generation and the map from another.
struct PluginStoreSnapshot {
    plugins: PluginMap,
    has_access_log_extern: bool,
}

impl PluginStoreSnapshot {
    fn new(plugins: PluginMap) -> Self {
        let has_access_log_extern = any_declares_access_log_extern(&plugins);
        Self {
            plugins,
            has_access_log_extern,
        }
    }
}

pub struct PluginStore {
    snapshot: ArcSwap<PluginStoreSnapshot>,
}

impl Default for PluginStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether any compiled plugin in the map declares a non-empty `accessLogExtern`.
fn any_declares_access_log_extern(map: &PluginMap) -> bool {
    map.values().any(|c| !c.spec.access_log_extern_fields().is_empty())
}

impl PluginStore {
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(PluginStoreSnapshot::new(HashMap::new())),
        }
    }

    /// Fast-path hint for `log_ext::collect`: true when at least one stored
    /// `EdgionPlugins` declares a non-empty `accessLogExtern`. When false, the
    /// caller can skip the plugin-store lookups entirely for this request.
    pub fn has_access_log_extern(&self) -> bool {
        self.snapshot.load().has_access_log_extern
    }

    /// Check if a plugin exists
    pub fn contains(&self, key: &str) -> bool {
        let snapshot = self.snapshot.load();
        snapshot.plugins.contains_key(key)
    }

    /// Execute a function with the compiled plugins reference
    pub fn with_plugin<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&CompiledPlugins) -> R,
    {
        let snapshot = self.snapshot.load();
        snapshot.plugins.get(key).map(|c| f(c))
    }

    /// Get the plugin runtime Arc for a given key
    pub fn get_plugin_runtime(&self, key: &str) -> Option<Arc<PluginRuntime>> {
        self.with_plugin(key, |c| Arc::clone(&c.plugin_runtime))
    }

    /// Replace all compiled plugins atomically, retaining a last-known-good
    /// runtime when a reconnect snapshot contains an invalid update for the
    /// same key. Keys absent from the new snapshot are still removed. Like
    /// [`Self::apply_delta`], this load-decide-store sequence relies on the
    /// serialized EdgionPlugins conf-sync writer.
    pub fn replace_all(&self, mut plugins: HashMap<String, Arc<CompiledPlugins>>) {
        {
            let current = self.snapshot.load();
            for (key, previous) in current.plugins.iter() {
                let Some(incoming) = plugins.get(key) else {
                    continue;
                };
                if incoming.retain_previous_on_invalid_update && previous.is_activatable {
                    plugins.insert(key.clone(), Arc::clone(previous));
                }
            }
        }
        self.publish(plugins);
    }

    /// Apply one serialized conf-sync delta and publish exactly one generation.
    ///
    /// `CacheData` invokes the EdgionPlugins handler while holding its
    /// per-resource-type write lock, so this load-clone-store sequence has one
    /// production writer. Adds replace unconditionally because a compressed
    /// add may represent delete-to-create for the same key. Last-known-good
    /// retention is therefore update-only.
    pub(crate) fn apply_delta(
        &self,
        add: HashMap<String, Arc<CompiledPlugins>>,
        update: HashMap<String, Arc<CompiledPlugins>>,
        remove: &HashSet<String>,
    ) {
        let mut new_map = {
            let current = self.snapshot.load();
            current.plugins.clone()
        };

        for key in remove {
            new_map.remove(key);
        }
        for (key, plugin) in add {
            new_map.insert(key, plugin);
        }
        for (key, plugin) in update {
            if plugin.retain_previous_on_invalid_update
                && new_map.get(&key).is_some_and(|previous| previous.is_activatable)
            {
                continue;
            }
            new_map.insert(key, plugin);
        }

        self.publish(new_map);
    }

    fn publish(&self, plugins: PluginMap) {
        self.snapshot.store(Arc::new(PluginStoreSnapshot::new(plugins)));
    }

    /// Get total count of compiled plugins
    pub fn count(&self) -> usize {
        let snapshot = self.snapshot.load();
        snapshot.plugins.len()
    }

    /// Per-kind plugin entry count across all stored EdgionPlugins resources.
    ///
    /// Discriminator: `EdgionPlugin::type_name()` (enum variant name, e.g. "JwtAuth", "BasicAuth").
    /// Each EdgionPlugins resource may declare plugins across four stages:
    /// requestPlugins, upstreamResponseFilterPlugins, upstreamResponseBodyFilterPlugins,
    /// upstreamResponsePlugins. All stages are counted.
    pub fn count_by_kind(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        let snapshot = self.snapshot.load();
        for compiled in snapshot.plugins.values() {
            let ep = &compiled.spec;
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
        let snapshot = self.snapshot.load();
        for (key, compiled) in snapshot.plugins.iter() {
            let ep = &compiled.spec;
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
        let snapshot = store.snapshot.load();
        assert_eq!(
            snapshot.has_access_log_extern,
            any_declares_access_log_extern(&snapshot.plugins)
        );
    }

    /// The flag must reflect what is currently *stored*, not merely what
    /// entered the store historically: no plugins, plugins without
    /// `accessLogExtern`, then adding one, then removing it.
    #[test]
    fn flag_flips_as_the_store_gains_and_loses_an_access_log_extern_declaration() {
        let store = PluginStore::new();
        assert!(!store.has_access_log_extern(), "empty store must report false");
        assert_snapshot_coherent(&store);

        let mut plugins = HashMap::new();
        plugins.insert(
            "ns/no-ext".to_string(),
            compile_plugins(&ep_without_access_log_extern()),
        );
        store.replace_all(plugins);
        assert!(
            !store.has_access_log_extern(),
            "a store with plugins but no accessLogExtern must report false"
        );
        assert_snapshot_coherent(&store);

        // Deterministic false-to-true transition.
        let mut add = HashMap::new();
        add.insert("ns/with-ext".to_string(), compile_plugins(&ep_with_access_log_extern()));
        store.apply_delta(add, HashMap::new(), &HashSet::new());
        assert!(
            store.has_access_log_extern(),
            "adding a resource with accessLogExtern must flip the flag to true"
        );
        assert_snapshot_coherent(&store);

        // Deterministic true-to-false transition.
        let mut remove = HashSet::new();
        remove.insert("ns/with-ext".to_string());
        store.apply_delta(HashMap::new(), HashMap::new(), &remove);
        assert!(
            !store.has_access_log_extern(),
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
            store.has_access_log_extern(),
            "replace_all with an accessLogExtern resource must set the flag"
        );

        // replace_all with an empty set must clear the flag.
        store.replace_all(HashMap::new());
        assert!(
            !store.has_access_log_extern(),
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
                        let snapshot = store.snapshot.load();
                        if snapshot.has_access_log_extern != any_declares_access_log_extern(&snapshot.plugins) {
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
                .with_plugin(&key, |published| std::ptr::eq(published, compiled.as_ref()))
                .unwrap());
            store.replace_all(HashMap::from([(key.clone(), Arc::clone(&compiled))]));
            assert!(store
                .with_plugin(&key, |published| std::ptr::eq(published, compiled.as_ref()))
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

        let snapshot = store.snapshot.load();
        let published = snapshot.plugins.get(&key).expect("updated generation is published");
        assert!(Arc::ptr_eq(published, &invalid));
        assert!(!Arc::ptr_eq(published, &valid));
        assert_leading_reject(published);
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
    fn inactive_and_policy_stamped_malformed_conditions_remain_inert() {
        for (name, mut entry) in [("disabled", body_dsl_entry()), ("policy bypass", body_dsl_entry())] {
            entry.conditions = Some(malformed_time_range_conditions());
            entry.body = Some(BodyRequirement {
                conditions: Some(bad_regex_conditions()),
                ..Default::default()
            });
            entry.dye = Some(response_dye(vec![malformed_time_range_conditions()]));
            if name == "disabled" {
                entry.enable = false;
            } else {
                entry.policy_action = Some(PluginPolicyAction::Bypass);
            }

            let resource = dsl_resource(entry);
            assert!(resource.validate_plugins().is_empty(), "{name}");
            let compiled = compile_plugins(&resource);
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
            assert!(Arc::ptr_eq(store.snapshot.load().plugins.get(&key).unwrap(), &next));
        }
        let disabled = resource("bad\0realm", false);
        assert!(disabled.validate_plugins().is_empty());
        assert!(compile_plugins(&disabled).is_activatable);
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
        store.apply_delta(
            HashMap::new(),
            HashMap::from([("default/native-waf".to_string(), Arc::clone(&invalid))]),
            &HashSet::new(),
        );

        let snapshot = store.snapshot.load();
        let retained = snapshot
            .plugins
            .get("default/native-waf")
            .expect("valid WAF generation remains published");
        assert!(Arc::ptr_eq(retained, &valid));
        assert!(!Arc::ptr_eq(retained, &invalid));
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

        let snapshot = store.snapshot.load();
        let replacement = snapshot.plugins.get(&key).expect("invalid add remains stored");
        assert!(Arc::ptr_eq(replacement, &invalid));
        assert!(!Arc::ptr_eq(replacement, &valid));
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
        assert!(Arc::ptr_eq(&in_flight_first, &first.plugin_runtime));
        assert!(Arc::ptr_eq(&in_flight_second, &second.plugin_runtime));
        assert!(!Arc::ptr_eq(&in_flight_first, &in_flight_second));

        store.replace_all(HashMap::from([(key.clone(), invalid)]));
        let retained = store
            .get_plugin_runtime(&key)
            .expect("invalid full snapshot retains the second generation");
        assert!(Arc::ptr_eq(&retained, &in_flight_second));

        store.apply_delta(
            HashMap::new(),
            HashMap::from([(key.clone(), Arc::clone(&recovered))]),
            &HashSet::new(),
        );
        let active_recovered = store
            .get_plugin_runtime(&key)
            .expect("recovered generation is published");
        assert!(Arc::ptr_eq(&active_recovered, &recovered.plugin_runtime));

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
                let first_runtime = Arc::clone(&first.plugin_runtime);
                let second_runtime = Arc::clone(&second.plugin_runtime);
                scope.spawn(move || {
                    for _ in 0..10_000 {
                        let runtime = store
                            .get_plugin_runtime(&key)
                            .expect("reload never removes the active WAF");
                        assert!(
                            Arc::ptr_eq(&runtime, &first_runtime) || Arc::ptr_eq(&runtime, &second_runtime),
                            "only complete valid generations may be observed"
                        );
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
        assert!(Arc::ptr_eq(&active, &first.plugin_runtime) || Arc::ptr_eq(&active, &second.plugin_runtime));
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
        fn policy_bypassed_invalid_json_schema() -> RequestFilterEntry {
            use edgion_resources::resources::edgion_plugins::JsonSchemaValidationConfig;
            use edgion_resources::resources::plugin_policy::PluginPolicyAction;

            let mut entry = json_schema_entry_with_body(Some("notasize"), true);
            entry.plugin = EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
                schema: serde_json::json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
            });
            entry.alias = Some("invalid alias".to_string());
            entry.conditions = disabled_with_bad_condition().conditions;
            entry.policy_action = Some(PluginPolicyAction::Bypass);
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
                // body. Both paths must report it with the identical message,
                // or the data plane silently forwards unguarded prompts.
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
                "policy_bypassed_invalid_json_schema_is_inert",
                mk(
                    Some(vec![
                        policy_bypassed_invalid_json_schema(),
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
