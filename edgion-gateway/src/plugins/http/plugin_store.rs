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

use crate::plugins::runtime::PluginRuntime;
use edgion_resources::resources::EdgionPlugins;

static GLOBAL_PLUGIN_STORE: LazyLock<Arc<PluginStore>> = LazyLock::new(|| Arc::new(PluginStore::new()));

pub fn get_global_plugin_store() -> Arc<PluginStore> {
    GLOBAL_PLUGIN_STORE.clone()
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
    /// Preparse/validation errors collected while building the runtime
    /// (surfaced into the resource's K8s status condition by the controller).
    pub preparse_errors: Vec<String>,
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
    let namespace = spec.metadata.namespace.clone().unwrap_or_else(|| "default".to_string());

    // Compile regex conditions on all plugin entries once, before building the
    // runtime. This avoids per-request Regex::new() in the fallback path of
    // condition evaluators. add_from_* clones the (now-compiled) conditions into
    // the runtime's ConditionalFilters.
    compile_plugin_conditions(&mut spec, &mut errors);

    let mut runtime = PluginRuntime::new();

    if let Some(request_plugins) = &spec.spec.request_plugins {
        errors.extend(runtime.add_from_request_filters(request_plugins, &namespace));
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

    // Fail closed: if any plugin (any stage) had an invalid config, prepend a
    // request-stage RejectFilter so every request through this EdgionPlugins is
    // rejected with 500 before reaching upstream, rather than silently running
    // with the misconfigured plugin absent from the chain.
    runtime.seal_fail_closed();

    Arc::new(CompiledPlugins {
        spec: Arc::new(spec),
        plugin_runtime: Arc::new(runtime),
        preparse_errors: errors,
    })
}

/// Compile regex patterns and validate static config in conditions for all
/// plugin entries. Both compile failures (regex) and validate failures (e.g.
/// malformed `timeRange` bounds) push into `errors`, which `compile_plugins`
/// then forwards to `seal_fail_closed`.
fn compile_plugin_conditions(ep: &mut EdgionPlugins, errors: &mut Vec<String>) {
    macro_rules! compile_entries {
        ($entries:expr, $label:expr) => {
            if let Some(entries) = &mut $entries {
                for (i, entry) in entries.iter_mut().enumerate() {
                    if let Some(conds) = entry.conditions.as_mut() {
                        if let Err(errs) = conds.compile() {
                            for e in errs {
                                let msg = format!("{}[{}] conditions regex: {}", $label, i, e);
                                tracing::warn!("{}", msg);
                                errors.push(msg);
                            }
                        }
                        if let Err(errs) = conds.validate() {
                            for e in errs {
                                let msg = format!("{}[{}] conditions: {}", $label, i, e);
                                tracing::warn!("{}", msg);
                                errors.push(msg);
                            }
                        }
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
}

/// Type alias for the compiled-plugin map (key: namespace/name)
type PluginMap = HashMap<String, Arc<CompiledPlugins>>;

pub struct PluginStore {
    plugins: ArcSwap<PluginMap>,
}

impl Default for PluginStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginStore {
    pub fn new() -> Self {
        Self {
            plugins: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if a plugin exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.plugins.load();
        map.contains_key(key)
    }

    /// Get the compiled plugins by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<Arc<CompiledPlugins>> {
        let map = self.plugins.load();
        map.get(key).cloned()
    }

    /// Execute a function with the compiled plugins reference
    pub fn with_plugin<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&CompiledPlugins) -> R,
    {
        let map = self.plugins.load();
        map.get(key).map(|c| f(c))
    }

    /// Get the plugin runtime Arc for a given key
    pub fn get_plugin_runtime(&self, key: &str) -> Option<Arc<PluginRuntime>> {
        self.with_plugin(key, |c| Arc::clone(&c.plugin_runtime))
    }

    /// Replace all compiled plugins atomically
    pub fn replace_all(&self, plugins: HashMap<String, Arc<CompiledPlugins>>) {
        self.plugins.store(Arc::new(plugins));
    }

    /// Update compiled plugins atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<CompiledPlugins>>, remove: &HashSet<String>) {
        let current = self.plugins.load();
        let current_map: &PluginMap = &current;
        let mut new_map: PluginMap = current_map.clone();

        for key in remove {
            new_map.remove(key);
        }
        for (key, plugin) in add_or_update {
            new_map.insert(key, plugin);
        }

        self.plugins.store(Arc::new(new_map));
    }

    /// Get total count of compiled plugins
    pub fn count(&self) -> usize {
        let map = self.plugins.load();
        map.len()
    }

    /// Per-kind plugin entry count across all stored EdgionPlugins resources.
    ///
    /// Discriminator: `EdgionPlugin::type_name()` (enum variant name, e.g. "JwtAuth", "BasicAuth").
    /// Each EdgionPlugins resource may declare plugins across four stages:
    /// requestPlugins, upstreamResponseFilterPlugins, upstreamResponseBodyFilterPlugins,
    /// upstreamResponsePlugins. All stages are counted.
    pub fn count_by_kind(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        let map = self.plugins.load();
        for compiled in map.values() {
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
        let map = self.plugins.load();
        for (key, compiled) in map.iter() {
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
mod compile_plugins_tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::{
        EdgionPlugin, EdgionPluginsSpec, RequestFilterEntry, UpstreamResponseFilterEntry,
    };
    use edgion_resources::resources::http_route::{HTTPHeader, HTTPHeaderFilter};

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
    /// lacks. DSL entries are intentionally excluded from the corpus — the
    /// documented divergence is that `validate_plugins` cannot run the
    /// gateway-only Rhai compile / VM-init deep checks.
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
        fn aliased(mut e: RequestFilterEntry, alias: &str) -> RequestFilterEntry {
            e.alias = Some(alias.to_string());
            e
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

        let cases: Vec<(&str, EdgionPlugins)> = vec![
            ("valid_request", mk(Some(vec![RequestFilterEntry::new(rhm())]), None)),
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
                "mixed_multi_stage",
                mk(
                    Some(vec![RequestFilterEntry::new(rhm()), RequestFilterEntry::new(respm())]),
                    Some(vec![
                        UpstreamResponseFilterEntry::new(respm()),
                        UpstreamResponseFilterEntry::new(rhm()),
                    ]),
                ),
            ),
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
