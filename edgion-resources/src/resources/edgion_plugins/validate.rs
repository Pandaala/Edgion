//! Schema-layer plugin validation, available to any consumer that needs the
//! EdgionPlugins preparse errors WITHOUT building the gateway plugin runtime.
//!
//! # Relationship to `edgion-gateway`'s `compile_plugins`
//!
//! The gateway's `compile_plugins` produces `preparse_errors` as a side effect
//! of building the `PluginRuntime`. The controller historically reached into the
//! gateway purely to collect those errors for the EdgionPlugins K8s status
//! condition, which forced a `controller -> gateway` crate dependency.
//!
//! [`EdgionPlugins::validate_plugins`] reproduces that error set at the schema
//! layer so the controller no longer needs the gateway. Since the DSL
//! compile/validate path was extracted into the `edgion-dsl` crate,
//! `DslConfig::validation_error()` runs the full static pipeline (compile
//! `source` / deserialize+validate `bytecode` / linkSysRefs / redis link-name
//! scan) — the same code path the gateway `DslPlugin` constructor uses. The
//! only remaining gateway-only DSL failure is VM init (`Vm::new`), which
//! cannot be predicted statically; the data plane still fails closed for it
//! at runtime.
//!
//! Every other error — condition regex compilation/validation, per-plugin
//! `validation_error()`, alias validation, and wrong-stage placement — is
//! reproduced with the same message text and ordering as `compile_plugins`.

use std::collections::BTreeSet;

use super::conditions::PluginConditions;
use super::edgion_plugin::{BodyStage, EdgionPlugin};
use super::entry::dye_set_clobber_error;
use super::EdgionPlugins;
use crate::resources::common::log_labels::LOG_LABEL_MAX_COUNT;

/// Per-variant stage applicability, mirroring the four
/// `PluginRuntime::create_*_from_edgion` matches in `edgion-gateway`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StageSet {
    pub request: bool,
    pub upstream_response_filter: bool,
    pub upstream_response_body_filter: bool,
    pub upstream_response: bool,
}

impl StageSet {
    /// The default shape: valid only in the request stage.
    pub const REQUEST_ONLY: StageSet = StageSet {
        request: true,
        upstream_response_filter: false,
        upstream_response_body_filter: false,
        upstream_response: false,
    };

    const fn new(
        request: bool,
        upstream_response_filter: bool,
        upstream_response_body_filter: bool,
        upstream_response: bool,
    ) -> StageSet {
        StageSet {
            request,
            upstream_response_filter,
            upstream_response_body_filter,
            upstream_response,
        }
    }
}

/// Plugin types that are NOT request-stage-only. Every `EdgionPlugin` variant
/// absent from this table is request-stage-only.
///
/// This is the single source of truth for stage applicability. It is keyed by
/// [`EdgionPlugin::type_name`] so the CRD drift guard can consume it directly
/// instead of maintaining its own copy — the mirror table that used to live in
/// `edgion-resources/tests/plugin_crd_type_coverage.rs` is gone.
///
/// Because this is a lookup rather than an exhaustive `match`, a new variant
/// silently defaults to request-only. That default is correct for the large
/// majority, and the case where it is not is caught two ways:
/// `stage_exception_keys_are_real_variants` proves every key here names a real
/// variant (read from `schema_for!(EdgionPlugin)`, i.e. the schema the CRD and
/// `plugin_policy` actually consume), and the CRD guard's Direction-3 proves
/// nothing reaches a response-stage CRD enum without being declared here.
pub const HTTP_PLUGIN_STAGE_EXCEPTIONS: &[(&str, StageSet)] = &[
    // Upstream-response-filter-stage-only plugins.
    ("ResponseHeaderModifier", StageSet::new(false, true, false, false)),
    ("DebugAccessLogToHeader", StageSet::new(false, true, false, false)),
    ("ResponseRewrite", StageSet::new(false, true, false, false)),
    // Upstream-response-body-filter-stage-only plugins.
    ("BandwidthLimit", StageSet::new(false, false, true, false)),
    // DSL runs in both the request and the upstream-response-filter stages.
    ("Dsl", StageSet::new(true, true, false, false)),
    // Wasm (proxy-wasm guest) runs in the request, upstream-response-filter,
    // and upstream-response-body-filter (read-only observation) stages,
    // matching its three CRD stage enum entries.
    ("Wasm", StageSet::new(true, true, true, false)),
    // One request-stage ExtProc entry owns request and response processing. The
    // gateway installs its response companion internally.
    ("ExtProc", StageSet::new(true, false, false, false)),
];

/// Which execution stages a plugin type name is valid for.
///
/// This mirrors `edgion-gateway`'s `create_*_from_edgion` stage gating: a
/// variant that those functions answer with `WrongStage` is `false` here. The
/// The async upstream-response stage currently has no directly configurable
/// plugin. ExtProc response processing is owned by its request-stage entry.
pub fn applicable_stages_by_name(type_name: &str) -> StageSet {
    let mut i = 0;
    while i < HTTP_PLUGIN_STAGE_EXCEPTIONS.len() {
        let (name, stages) = HTTP_PLUGIN_STAGE_EXCEPTIONS[i];
        if name.as_bytes() == type_name.as_bytes() {
            return stages;
        }
        i += 1;
    }
    StageSet::REQUEST_ONLY
}

/// Which execution stages a plugin variant is valid for.
fn applicable_stages(plugin: &EdgionPlugin) -> StageSet {
    applicable_stages_by_name(plugin.type_name())
}

/// Compile + validate the regex conditions on a single entry, pushing any
/// errors with the same wording the gateway's `compile_plugin_conditions` uses.
/// `compile()` mutates in place, so we operate on a clone (only the errors are
/// wanted here).
fn collect_condition_errors(conds: &PluginConditions, label: &str, index: usize, errors: &mut Vec<String>) {
    let mut conds = conds.clone();
    if let Err(errs) = conds.compile() {
        for e in errs {
            errors.push(format!("{}[{}] conditions regex: {}", label, index, e));
        }
    }
    if let Err(errs) = conds.validate() {
        for e in errs {
            errors.push(format!("{}[{}] conditions: {}", label, index, e));
        }
    }
}

/// Same as [`collect_condition_errors`] but for a plugin's secondary body gate
/// conditions (`entry.body.conditions`) — mirrors the gateway's
/// `compile_body_entries` wording (`"... body conditions ..."`).
fn collect_body_condition_errors(conds: &PluginConditions, label: &str, index: usize, errors: &mut Vec<String>) {
    let mut conds = conds.clone();
    if let Err(errs) = conds.compile() {
        for e in errs {
            errors.push(format!("{}[{}] body conditions regex: {}", label, index, e));
        }
    }
    if let Err(errs) = conds.validate() {
        for e in errs {
            errors.push(format!("{}[{}] body conditions: {}", label, index, e));
        }
    }
}

impl EdgionPlugins {
    /// Collect the preparse validation errors for this resource WITHOUT building
    /// the gateway plugin runtime.
    ///
    /// Mirrors the `preparse_errors` that `edgion-gateway`'s `compile_plugins`
    /// surfaces into the EdgionPlugins K8s status condition, minus DSL VM init
    /// (`Vm::new`), the one failure that cannot be predicted statically. See
    /// the module docs for the full contract.
    pub fn validate_plugins(&self) -> Vec<String> {
        let mut errors = Vec::new();

        // Pass 1: condition regex compilation/validation for active entries,
        // in stage order — mirrors `compile_plugin_conditions`. Disabled and
        // PluginPolicy-stamped entries are semantically inert: the gateway
        // never builds or runs them, so dormant conditions must not flip the
        // resource to `Accepted=False`. Skipping here keeps the schema layer
        // and gateway in lockstep (parity test in `plugin_store.rs`).
        macro_rules! compile_conditions {
            ($entries:expr, $label:literal) => {
                if let Some(entries) = &$entries {
                    for (i, entry) in entries.iter().enumerate() {
                        if !entry.is_enabled() || entry.policy_action.is_some() {
                            continue;
                        }
                        if let Some(conds) = entry.conditions.as_ref() {
                            collect_condition_errors(conds, $label, i, &mut errors);
                        }
                    }
                }
            };
        }
        compile_conditions!(self.spec.request_plugins, "requestPlugins");
        compile_conditions!(
            self.spec.upstream_response_filter_plugins,
            "upstreamResponseFilterPlugins"
        );
        compile_conditions!(
            self.spec.upstream_response_body_filter_plugins,
            "upstreamResponseBodyFilterPlugins"
        );
        compile_conditions!(self.spec.upstream_response_plugins, "upstreamResponsePlugins");

        // Secondary body-gate conditions (`entry.body.conditions`) — only the two
        // async stages carry `body`. Mirrors the gateway `compile_body_entries`.
        macro_rules! compile_body_conditions {
            ($entries:expr, $label:literal) => {
                if let Some(entries) = &$entries {
                    for (i, entry) in entries.iter().enumerate() {
                        // Inert entries are skipped, matching the entry-level
                        // conditions pass and gateway `compile_body_entries`.
                        if !entry.is_enabled() || entry.policy_action.is_some() {
                            continue;
                        }
                        if let Some(conds) = entry.body.as_ref().and_then(|b| b.conditions.as_ref()) {
                            collect_body_condition_errors(conds, $label, i, &mut errors);
                        }
                    }
                }
            };
        }
        compile_body_conditions!(self.spec.request_plugins, "requestPlugins");
        compile_body_conditions!(self.spec.upstream_response_plugins, "upstreamResponsePlugins");

        // Request-stage entries whose bounded-structured `body` is invalid. The
        // gateway's `add_from_request_filters` records these in its prepass and
        // then skips the entry entirely, so Pass 2 must skip them too — otherwise
        // the two layers disagree on which entries reach the alias / dye / stage
        // checks (and on which entries claim an alias for the uniqueness rule).
        let bounded_structured_invalid: std::collections::HashSet<usize> = self
            .spec
            .request_plugins
            .as_ref()
            .map(|entries| {
                entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        entry.is_enabled()
                            && entry.policy_action.is_none()
                            && entry.plugin.validation_error().is_none()
                            && matches!(
                                entry.plugin,
                                EdgionPlugin::FormJsonTransform(_)
                                    | EdgionPlugin::JsonSchemaValidation(_)
                                    | EdgionPlugin::ProxyRewrite(super::plugin_configs::ProxyRewriteConfig {
                                        json_body: Some(_),
                                        ..
                                    })
                            )
                            && entry.body.as_ref().is_some_and(|body| {
                                !super::plugin_configs::bounded_structured_body_validation_errors(body).is_empty()
                            })
                    })
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();

        // Pass 2: per-entry config/alias/stage validation for active entries,
        // in stage order — mirrors the four `add_from_*` paths (minus runtime
        // construction and the DSL deep checks).
        macro_rules! validate_entries {
            ($entries:expr, $label:literal, $stage_word:literal, $stage_field:ident) => {
                if let Some(entries) = &$entries {
                    // Per-stage-array alias accumulator; see `duplicate_alias_error`.
                    let mut seen_aliases = std::collections::HashSet::new();
                    for (i, entry) in entries.iter().enumerate() {
                        if !entry.is_enabled() || entry.policy_action.is_some() {
                            continue;
                        }
                        let name = entry.plugin.type_name();
                        // 1. Per-plugin config validation. On error, skip the
                        //    remaining checks (mirrors the gateway `continue`).
                        if let Some(err) = entry.plugin.validation_error() {
                            errors.push(format!("{}[{}] ({}): {}", $label, i, name, err));
                            continue;
                        }
                        // 1b. Mirror the gateway's bounded-structured-body prepass
                        //     (`add_from_request_filters`): such an error owns the entry,
                        //     which is then skipped before its alias / dye / stage checks.
                        //     Without this the two layers disagree on which entries claim
                        //     an alias, and the uniqueness rule below would drift.
                        if matches!($stage_word, "request") && bounded_structured_invalid.contains(&i) {
                            continue;
                        }
                        if !matches!($stage_word, "request") {
                            if let EdgionPlugin::Dsl(config) = &entry.plugin {
                                if config
                                    .compile_and_validate()
                                    .is_ok_and(|script| script.references_request_body_mutation())
                                {
                                    errors.push(format!(
                                        "{}[{}] ({}): request-body mutation is only valid in request stage",
                                        $label, i, name
                                    ));
                                    continue;
                                }
                            }
                        }
                        // 2. Optional alias. Recorded but does NOT skip the stage
                        //    check — the gateway only drops the invalid alias.
                        if let Some(err) = entry.validate_alias() {
                            errors.push(format!("{}[{}] ({}): {}", $label, i, name, err));
                        } else if let Some(err) = crate::resources::edgion_plugins::duplicate_alias_error(
                            &mut seen_aliases,
                            entry.alias.as_deref(),
                        ) {
                            // 2a. Alias uniqueness within this stage array. The alias
                            //     is part of the entry's runtime identity, so a
                            //     duplicate silently merges two plugins' external
                            //     state (RateLimitRedis buckets) and their log label.
                            errors.push(format!("{}[{}] ({}): {}", $label, i, name, err));
                        }
                        // 2b. Optional entry-level dye suite. Same severity as the
                        //     alias check above: recorded, does not skip the stage
                        //     check.
                        if let Some(err) = entry.validate_dye() {
                            errors.push(format!("{}[{}] ({}): {}", $label, i, name, err));
                        }
                        // 3. Wrong-stage placement.
                        if !applicable_stages(&entry.plugin).$stage_field {
                            errors.push(format!(
                                "{}[{}] ({}): plugin is not valid for {} stage",
                                $label, i, name, $stage_word
                            ));
                        }
                    }
                }
            };
        }
        validate_entries!(self.spec.request_plugins, "requestPlugins", "request", request);
        validate_entries!(
            self.spec.upstream_response_filter_plugins,
            "upstreamResponseFilterPlugins",
            "upstream response filter",
            upstream_response_filter
        );
        validate_entries!(
            self.spec.upstream_response_body_filter_plugins,
            "upstreamResponseBodyFilterPlugins",
            "upstream response body filter",
            upstream_response_body_filter
        );
        validate_entries!(
            self.spec.upstream_response_plugins,
            "upstreamResponsePlugins",
            "upstream response",
            upstream_response
        );

        // Pass 2b: resource-level dye "set clobber" check. A failure rule's
        // response header must not equal (case-insensitively) any success
        // rule's response header across ANY entry in this resource — success
        // renders with Set semantics and would silently wipe an appended
        // failure value queued earlier by a different entry. Runs once over
        // all four stage arrays chained (only active entries participate,
        // matching the inert-entry rule used throughout this pass).
        let all_dye_suites = self
            .spec
            .request_plugins
            .iter()
            .flatten()
            .filter(|e| e.is_enabled() && e.policy_action.is_none())
            .map(|e| e.dye.as_ref())
            .chain(
                self.spec
                    .upstream_response_filter_plugins
                    .iter()
                    .flatten()
                    .filter(|e| e.is_enabled() && e.policy_action.is_none())
                    .map(|e| e.dye.as_ref()),
            )
            .chain(
                self.spec
                    .upstream_response_body_filter_plugins
                    .iter()
                    .flatten()
                    .filter(|e| e.is_enabled() && e.policy_action.is_none())
                    .map(|e| e.dye.as_ref()),
            )
            .chain(
                self.spec
                    .upstream_response_plugins
                    .iter()
                    .flatten()
                    .filter(|e| e.is_enabled() && e.policy_action.is_none())
                    .map(|e| e.dye.as_ref()),
            );
        if let Some(err) = dye_set_clobber_error(all_dye_suites) {
            errors.push(err);
        }

        // Pass 2c: operator body requirement validation for the two async stages
        // that carry a `body` field. Mirrors the gateway `add_from_*` paths, which
        // check `entry.body` only for enabled, config-valid entries (a config
        // error `continue`s before the body check), with the same
        // `"...: body: <err>"` messages — kept in lockstep by the drift test.
        macro_rules! validate_body_entries {
            ($entries:expr, $label:literal, $stage:expr) => {
                if let Some(entries) = &$entries {
                    for (i, entry) in entries.iter().enumerate() {
                        if !entry.is_enabled()
                            || entry.policy_action.is_some()
                            || entry.plugin.validation_error().is_some()
                        {
                            continue;
                        }
                        let Some(body) = entry.body.as_ref() else { continue };
                        let name = entry.plugin.type_name();
                        // `body:` only applies to plugins that consume the body in
                        // their current config; reject it elsewhere so it is never
                        // a silent no-op.
                        if !entry.plugin.accepts_body($stage) {
                            errors.push(format!(
                                "{}[{}] ({}): body: plugin does not consume a body",
                                $label, i, name
                            ));
                        } else if matches!(
                            entry.plugin,
                            EdgionPlugin::FormJsonTransform(_)
                                | EdgionPlugin::JsonSchemaValidation(_)
                                | EdgionPlugin::ProxyRewrite(_)
                        ) {
                            for error in super::plugin_configs::bounded_structured_body_validation_errors(body) {
                                errors.push(format!("{}[{}] ({}): {}", $label, i, name, error));
                            }
                        } else if let Some(err) = body.validation_error() {
                            errors.push(format!("{}[{}] ({}): body: {}", $label, i, name, err));
                        }
                    }
                }
            };
        }
        validate_body_entries!(self.spec.request_plugins, "requestPlugins", BodyStage::Request);
        validate_body_entries!(
            self.spec.upstream_response_plugins,
            "upstreamResponsePlugins",
            BodyStage::Response
        );

        if let Some(entries) = &self.spec.request_plugins {
            // Inert entries do not participate in cardinality.
            let json_schema_count = entries
                .iter()
                .filter(|entry| {
                    entry.is_enabled()
                        && entry.policy_action.is_none()
                        && matches!(entry.plugin, EdgionPlugin::JsonSchemaValidation(_))
                })
                .count();
            if json_schema_count > 1 {
                errors.push(format!(
                    "requestPlugins: at most one JsonSchemaValidation plugin is allowed, found {json_schema_count}"
                ));
            }

            // Multiple active TraceContext entries would each overwrite
            // x-trace-id (last-write-wins), which is confusing.
            let count = entries
                .iter()
                .filter(|e| e.is_enabled() && e.policy_action.is_none() && e.plugin.type_name() == "TraceContext")
                .count();
            if count > 1 {
                errors.push(format!(
                    "requestPlugins: at most one TraceContext plugin is allowed, found {count}"
                ));
            }
        }

        // Pass 3: access log extension declarations. Not a stage — no
        // `applicable_stages` check applies. Per-entry config validity, plus a
        // resource-level uniqueness and count cap: a duplicate `key` would
        // silently last-win on YAML order, and an over-cap list would be
        // silently truncated on the data path.
        if let Some(fields) = &self.spec.access_log_extern {
            if fields.len() > LOG_LABEL_MAX_COUNT {
                errors.push(format!(
                    "accessLogExtern: at most {LOG_LABEL_MAX_COUNT} fields per resource (got {})",
                    fields.len()
                ));
            }
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for (i, field) in fields.iter().enumerate() {
                if let Some(err) = field.validation_error() {
                    errors.push(format!("accessLogExtern[{i}]: {err}"));
                    continue;
                }
                if !seen.insert(field.key.as_str()) {
                    errors.push(format!("accessLogExtern[{i}]: duplicate key '{}'", field.key));
                }
            }
        }

        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::{
        DslConfig, FormJsonDirection, FormJsonTransformConfig, JsonSchemaValidationConfig, WasmConfig, WasmRawConfig,
        MAX_WASM_RAW_CONFIG_BYTES,
    };
    use crate::resources::edgion_plugins::{
        BodyReadFailureMode, BodyRequirement, EdgionPluginsSpec, RequestFilterEntry, UpstreamResponseFilterEntry,
        ABSOLUTE_INSTANCE_MAX_BYTES, DEFAULT_INSTANCE_MAX_BYTES,
    };
    use crate::resources::http_route::HTTPHeaderFilter;
    use serde_json::json;

    fn header_filter() -> HTTPHeaderFilter {
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: None,
        }
    }

    fn aliased_request_entry(alias: &str) -> RequestFilterEntry {
        let mut entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
        entry.alias = Some(alias.to_string());
        entry
    }

    #[test]
    fn duplicate_alias_in_one_stage_is_rejected() {
        let ep = with_request_plugins(vec![aliased_request_entry("dup"), aliased_request_entry("dup")]);
        let errors = ep.validate_plugins();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("requestPlugins[1] "), "{errors:?}");
        assert!(errors[0].contains("duplicate alias 'dup'"), "{errors:?}");
    }

    #[test]
    fn distinct_aliases_in_one_stage_are_accepted() {
        let ep = with_request_plugins(vec![aliased_request_entry("a"), aliased_request_entry("b")]);
        assert!(ep.validate_plugins().is_empty());
    }

    #[test]
    fn bounded_body_invalid_entry_does_not_claim_its_alias() {
        // Mirrors the gateway prepass: a bounded-structured-body error owns the
        // entry, so it never reaches the alias check and a later entry reusing
        // that alias is NOT a duplicate. Without the skip the two validation
        // layers would disagree on this input.
        use crate::resources::edgion_plugins::BodyRequirement;

        let mut first = RequestFilterEntry::new(EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
            schema: json!({"type": "object"}),
        }));
        first.alias = Some("dup".to_string());
        first.body = Some(BodyRequirement {
            max_body_size: Some("notasize".to_string()),
            ..Default::default()
        });

        let errors = with_request_plugins(vec![first, aliased_request_entry("dup")]).validate_plugins();

        assert!(
            errors.iter().any(|e| e.contains("maxBodySize")),
            "the bounded-body error must still be reported: {errors:?}"
        );
        assert!(
            !errors.iter().any(|e| e.contains("duplicate alias")),
            "a skipped entry must not claim its alias: {errors:?}"
        );
    }

    #[test]
    fn duplicate_alias_on_a_disabled_entry_is_ignored() {
        // Inert entries are never built, so their alias claims nothing.
        let mut disabled = aliased_request_entry("dup");
        disabled.enable = false;
        let ep = with_request_plugins(vec![disabled, aliased_request_entry("dup")]);
        assert!(ep.validate_plugins().is_empty());
    }

    fn with_request_plugins(entries: Vec<RequestFilterEntry>) -> EdgionPlugins {
        EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                request_plugins: Some(entries),
                ..Default::default()
            },
            status: None,
        }
    }

    #[test]
    fn controller_validate_plugins_matches_wasm_raw_config_validation() {
        let mut config = WasmConfig::default();
        config.source.file = Some("./demo.wasm".to_string());
        config.plugin_config = Some(WasmRawConfig::base64("/wA="));
        let valid = RequestFilterEntry::new(EdgionPlugin::Wasm(config.clone()));
        assert!(with_request_plugins(vec![valid]).validate_plugins().is_empty());

        config.plugin_config = Some(WasmRawConfig::string("x".repeat(MAX_WASM_RAW_CONFIG_BYTES + 1)));
        let invalid = RequestFilterEntry::new(EdgionPlugin::Wasm(config));
        let errors = with_request_plugins(vec![invalid]).validate_plugins();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("pluginConfig decoded value exceeds the 64KiB limit")),
            "{errors:?}"
        );
    }

    fn dsl_entry(source: &str) -> RequestFilterEntry {
        RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "d".to_string(),
            source: Some(source.to_string()),
            ..Default::default()
        }))
    }

    #[test]
    fn body_on_non_consuming_plugin_is_rejected() {
        use crate::resources::edgion_plugins::BodyRequirement;
        let mut entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
        entry.body = Some(BodyRequirement::fail_close());
        let errs = with_request_plugins(vec![entry]).validate_plugins();
        assert!(
            errs.iter().any(|e| e.contains("body: plugin does not consume a body")),
            "expected 'does not consume a body', got: {errs:?}"
        );
    }

    #[test]
    fn bad_body_max_size_on_consuming_plugin_is_flagged() {
        use crate::resources::edgion_plugins::{BodyRequirement, HmacAuthConfig};
        let cfg = HmacAuthConfig {
            validate_request_body: true,
            anonymous: Some("anon".to_string()),
            ..Default::default()
        };
        let mut entry = RequestFilterEntry::new(EdgionPlugin::HmacAuth(cfg));
        entry.body = Some(BodyRequirement {
            max_body_size: Some("notasize".to_string()),
            ..Default::default()
        });
        let errs = with_request_plugins(vec![entry]).validate_plugins();
        assert!(
            errs.iter().any(|e| e.contains("body: invalid body.maxBodySize")),
            "expected a maxBodySize error, got: {errs:?}"
        );
    }

    #[test]
    fn valid_body_on_consuming_plugin_has_no_error() {
        use crate::resources::edgion_plugins::{BodyRequirement, HmacAuthConfig};
        let cfg = HmacAuthConfig {
            validate_request_body: true,
            anonymous: Some("anon".to_string()),
            ..Default::default()
        };
        let mut entry = RequestFilterEntry::new(EdgionPlugin::HmacAuth(cfg));
        entry.body = Some(BodyRequirement {
            max_body_size: Some("1m".to_string()),
            ..Default::default()
        });
        assert!(with_request_plugins(vec![entry]).validate_plugins().is_empty());
    }

    fn json_schema_entry() -> RequestFilterEntry {
        RequestFilterEntry::new(EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
            schema: json!({"type": "object"}),
        }))
    }

    fn form_json_entry() -> RequestFilterEntry {
        RequestFilterEntry::new(EdgionPlugin::FormJsonTransform(FormJsonTransformConfig {
            direction: FormJsonDirection::FormToJson,
        }))
    }

    #[test]
    fn form_json_transform_defaults_to_bounded_fail_close_request_body() {
        let entry = form_json_entry();
        let intrinsic = entry.plugin.body_requirement(BodyStage::Request).unwrap();
        assert_eq!(intrinsic.max_body_size_bytes(), Some(Ok(DEFAULT_INSTANCE_MAX_BYTES)));
        assert!(intrinsic.is_fail_close());
        assert!(intrinsic.conditions.is_none());
        assert!(entry.plugin.body_requirement(BodyStage::Response).is_none());
    }

    #[test]
    fn form_json_transform_rejects_conditions_fail_open_and_oversized_limit() {
        let mut entry = form_json_entry();
        entry.body = Some(BodyRequirement {
            max_body_size: Some((ABSOLUTE_INSTANCE_MAX_BYTES + 1).to_string()),
            conditions: Some(Default::default()),
            on_read_failure: BodyReadFailureMode::FailOpen,
        });
        let errors = with_request_plugins(vec![entry]).validate_plugins();
        for expected in [
            "conditions are not supported",
            "body.onReadFailure must be failClose",
            "must not exceed 1MiB",
        ] {
            assert!(
                errors.iter().any(|error| error.contains(expected)),
                "missing {expected:?}: {errors:?}"
            );
        }
    }

    #[test]
    fn form_json_transform_is_request_stage_only() {
        let entry = UpstreamResponseFilterEntry::new(form_json_entry().plugin);
        let plugins = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                upstream_response_filter_plugins: Some(vec![entry]),
                ..Default::default()
            },
            status: None,
        };
        let errors = plugins.validate_plugins();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("plugin is not valid for upstream response filter stage")),
            "{errors:?}"
        );
    }

    #[test]
    fn json_schema_intrinsic_body_limit_defaults_to_64_kib() {
        let entry = json_schema_entry();
        let intrinsic = entry.plugin.body_requirement(BodyStage::Request).unwrap();
        assert_eq!(intrinsic.max_body_size_bytes(), Some(Ok(DEFAULT_INSTANCE_MAX_BYTES)));

        let operator = BodyRequirement {
            on_read_failure: BodyReadFailureMode::FailOpen,
            ..Default::default()
        };
        let resolved = BodyRequirement::resolve(Some(operator), Some(intrinsic)).unwrap();
        assert_eq!(resolved.max_body_size_bytes(), Some(Ok(DEFAULT_INSTANCE_MAX_BYTES)));
        assert!(resolved.is_fail_close());
    }

    #[test]
    fn json_schema_body_policy_rejects_conditions_and_limit_above_1_mib() {
        let mut entry = json_schema_entry();
        entry.body = Some(BodyRequirement {
            max_body_size: Some((ABSOLUTE_INSTANCE_MAX_BYTES + 1).to_string()),
            conditions: Some(Default::default()),
            ..Default::default()
        });
        let errors = with_request_plugins(vec![entry]).validate_plugins();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("conditions are not supported")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("must not exceed 1MiB")),
            "{errors:?}"
        );
    }

    #[test]
    fn json_schema_body_policy_rejects_invalid_and_zero_limits() {
        for limit in ["notasize", "0"] {
            let mut entry = json_schema_entry();
            entry.body = Some(BodyRequirement {
                max_body_size: Some(limit.to_string()),
                ..Default::default()
            });
            let errors = with_request_plugins(vec![entry]).validate_plugins();
            assert!(
                errors.iter().any(|error| error.contains("body.maxBodySize")),
                "limit {limit:?} should fail: {errors:?}"
            );
        }
    }

    #[test]
    fn json_schema_body_limit_allows_exactly_1_mib() {
        let mut entry = json_schema_entry();
        entry.body = Some(BodyRequirement {
            max_body_size: Some(ABSOLUTE_INSTANCE_MAX_BYTES.to_string()),
            ..Default::default()
        });
        assert!(with_request_plugins(vec![entry]).validate_plugins().is_empty());
    }

    #[test]
    fn rejects_multiple_json_schema_plugins_in_one_request_list() {
        let errors = with_request_plugins(vec![json_schema_entry(), json_schema_entry()]).validate_plugins();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("at most one JsonSchemaValidation")),
            "{errors:?}"
        );
    }

    #[test]
    fn disabled_json_schema_plugin_is_inert_for_cardinality() {
        let enabled = json_schema_entry();
        let disabled = RequestFilterEntry::with_enable(json_schema_entry().plugin, false);
        let errors = with_request_plugins(vec![enabled, disabled]).validate_plugins();
        assert!(
            !errors
                .iter()
                .any(|error| error.contains("at most one JsonSchemaValidation")),
            "{errors:?}"
        );
    }

    #[test]
    fn policy_stamped_json_schema_plugin_is_inert_for_all_validation_passes() {
        use crate::resources::plugin_policy::PluginPolicyAction;

        let mut bypassed = json_schema_entry();
        bypassed.policy_action = Some(PluginPolicyAction::Bypass);
        bypassed.alias = Some("invalid alias".to_string());
        bypassed.conditions = Some(
            serde_json::from_value(json!({
                "run": [{
                    "type": "keyMatch",
                    "key": {"type": "header", "name": "X-Test"},
                    "regex": ["("]
                }]
            }))
            .unwrap(),
        );
        bypassed.body = Some(BodyRequirement {
            max_body_size: Some("invalid".to_string()),
            conditions: Some(Default::default()),
            ..Default::default()
        });
        bypassed.plugin = EdgionPlugin::JsonSchemaValidation(JsonSchemaValidationConfig {
            schema: json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
        });

        let errors = with_request_plugins(vec![bypassed, json_schema_entry()]).validate_plugins();
        assert!(errors.is_empty(), "policy-stamped entries must be inert: {errors:?}");
    }

    #[test]
    fn json_schema_plugin_is_request_stage_only() {
        let entry = UpstreamResponseFilterEntry::new(json_schema_entry().plugin);
        let plugins = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                upstream_response_filter_plugins: Some(vec![entry]),
                ..Default::default()
            },
            status: None,
        };
        let errors = plugins.validate_plugins();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("plugin is not valid for upstream response filter stage")),
            "{errors:?}"
        );
    }

    #[test]
    fn dsl_bad_syntax_surfaces_at_schema_layer() {
        let ep = with_request_plugins(vec![dsl_entry("this is not valid dsl @@@")]);
        let errs = ep.validate_plugins();
        assert!(
            errs.iter().any(|e| e.contains("DSL compile errors")),
            "expected a DSL compile error, got: {errs:?}"
        );
    }

    #[test]
    fn dsl_wrong_builtin_arity_surfaces_at_schema_layer() {
        // req.header takes exactly 1 argument; arity is validated at compile time.
        let ep = with_request_plugins(vec![dsl_entry("let h = req.header()")]);
        let errs = ep.validate_plugins();
        assert!(
            errs.iter()
                .any(|e| e.contains("DSL compile errors") && e.contains("expects 1 argument(s), got 0")),
            "expected an arity compile error, got: {errs:?}"
        );
    }

    #[test]
    fn dsl_unknown_redis_link_surfaces_at_schema_layer() {
        let ep = with_request_plugins(vec![dsl_entry("let r = redis.get(\"mylink\", \"mykey\")")]);
        let errs = ep.validate_plugins();
        assert!(
            errs.iter().any(|e| e.contains("linkSysRefs")),
            "expected a redis link-name error, got: {errs:?}"
        );
    }

    #[test]
    fn dsl_request_body_mutation_is_rejected_in_response_stage() {
        let entry = UpstreamResponseFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "late-mutation".to_string(),
            source: Some("req.body.clear()".to_string()),
            ..Default::default()
        }));
        let plugins = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                upstream_response_filter_plugins: Some(vec![entry]),
                ..Default::default()
            },
            status: None,
        };
        let errors = plugins.validate_plugins();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("request-body mutation is only valid in request stage")),
            "{errors:?}"
        );
    }

    #[test]
    fn dsl_request_body_read_is_allowed_in_response_stage() {
        let entry = UpstreamResponseFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "late-read".to_string(),
            source: Some("let found = req.body.contains(\"needle\")".to_string()),
            ..Default::default()
        }));
        let plugins = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                upstream_response_filter_plugins: Some(vec![entry]),
                ..Default::default()
            },
            status: None,
        };
        assert!(plugins.validate_plugins().is_empty());
    }

    #[test]
    fn valid_request_plugin_produces_no_errors() {
        let ep = with_request_plugins(vec![RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
            header_filter(),
        ))]);
        assert!(ep.validate_plugins().is_empty());
    }

    #[test]
    fn response_only_plugin_in_request_stage_is_wrong_stage() {
        // ResponseHeaderModifier has no preparse validation_error, so the only
        // error surfaced is the wrong-stage placement.
        let ep = with_request_plugins(vec![RequestFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
            header_filter(),
        ))]);
        assert_eq!(
            ep.validate_plugins(),
            vec!["requestPlugins[0] (ResponseHeaderModifier): plugin is not valid for request stage".to_string()]
        );
    }

    #[test]
    fn disabled_entry_is_skipped() {
        let ep = with_request_plugins(vec![RequestFilterEntry::with_enable(
            EdgionPlugin::ResponseHeaderModifier(header_filter()),
            false,
        )]);
        assert!(ep.validate_plugins().is_empty());
    }

    #[test]
    fn invalid_alias_is_reported_and_does_not_skip_stage_check() {
        // A valid, correctly-placed plugin with a bad alias yields exactly the
        // alias error (and no spurious stage error).
        let mut entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
        entry.alias = Some("bad alias".to_string());
        let ep = with_request_plugins(vec![entry]);
        assert_eq!(
            ep.validate_plugins(),
            vec![concat!(
                "requestPlugins[0] (RequestHeaderModifier): ",
                "alias must match [a-zA-Z0-9._-]{1,32} (contains disallowed character)"
            )
            .to_string()]
        );
    }
}

#[cfg(test)]
mod disabled_condition_tests {
    use crate::resources::edgion_plugins::EdgionPlugins;

    fn validate(yaml: &str) -> Vec<String> {
        let plugins: EdgionPlugins = serde_yaml::from_str(yaml).expect("deserialize EdgionPlugins");
        plugins.validate_plugins()
    }

    /// A disabled entry whose dormant `conditions` carry a malformed regex must
    /// not produce any validation error — `enable: false` is semantically inert.
    #[test]
    fn disabled_entry_with_malformed_conditions_is_inert() {
        let errs = validate(
            r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: parked
  namespace: default
spec:
  requestPlugins:
    - type: RequestHeaderModifier
      enable: false
      config: {}
      conditions:
        run:
          - type: keyMatch
            key:
              type: header
              name: X-Test
            regex: ["("]
"#,
        );
        assert!(
            errs.is_empty(),
            "disabled entry must not surface a condition error, got: {errs:?}"
        );
    }

    /// Enabling the same malformed entry deterministically surfaces the
    /// condition regex error — the disabled/enabled boundary is the only switch.
    #[test]
    fn enabling_the_same_entry_surfaces_the_condition_error() {
        let errs = validate(
            r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: active
  namespace: default
spec:
  requestPlugins:
    - type: RequestHeaderModifier
      enable: true
      config: {}
      conditions:
        run:
          - type: keyMatch
            key:
              type: header
              name: X-Test
            regex: ["("]
"#,
        );
        assert!(
            errs.iter().any(|e| e.contains("conditions regex")),
            "enabled entry must surface the condition regex error, got: {errs:?}"
        );
    }
}

#[cfg(test)]
mod trace_context_dup_tests {
    use crate::resources::edgion_plugins::EdgionPlugins;

    fn validate(yaml: &str) -> Vec<String> {
        let plugins: EdgionPlugins = serde_yaml::from_str(yaml).expect("deserialize EdgionPlugins");
        plugins.validate_plugins()
    }

    #[test]
    fn rejects_two_trace_context() {
        let errs = validate(
            r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dup
  namespace: default
spec:
  requestPlugins:
    - type: TraceContext
      config: {}
    - type: TraceContext
      config: {}
"#,
        );
        assert!(
            errs.iter().any(|e| e.contains("at most one TraceContext")),
            "expected duplicate error, got: {errs:?}"
        );
    }

    #[test]
    fn allows_single_trace_context() {
        let errs = validate(
            r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ok
  namespace: default
spec:
  requestPlugins:
    - type: TraceContext
      config: {}
"#,
        );
        assert!(
            !errs.iter().any(|e| e.contains("at most one TraceContext")),
            "unexpected duplicate error: {errs:?}"
        );
    }
}

#[cfg(test)]
mod dye_validate_tests {
    use crate::resources::edgion_plugins::dye::{DyeOutcome, DyeRule, DyeSuite};
    use crate::resources::edgion_plugins::{
        EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry, UpstreamResponseFilterEntry,
    };

    fn with_request_plugins(entries: Vec<RequestFilterEntry>) -> EdgionPlugins {
        EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                request_plugins: Some(entries),
                ..Default::default()
            },
            status: None,
        }
    }

    fn header_filter() -> crate::resources::http_route::HTTPHeaderFilter {
        crate::resources::http_route::HTTPHeaderFilter {
            set: None,
            add: None,
            remove: None,
        }
    }

    fn dye_response(name: &str, on: &[DyeOutcome]) -> DyeSuite {
        DyeSuite {
            request: vec![],
            response: vec![DyeRule {
                name: name.to_string(),
                on: on.to_vec(),
                value: None,
                conditions: None,
            }],
        }
    }

    #[test]
    fn invalid_dye_is_reported_and_does_not_skip_stage_check() {
        // RequestHeaderModifier declares no dye capabilities, so a default
        // `[success]` dye rule is rejected — mirrors the alias parity test
        // above, with no spurious stage error alongside it.
        let mut entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
        entry.dye = Some(dye_response("X-Dye", &[DyeOutcome::Success]));
        let ep = with_request_plugins(vec![entry]);
        let errs = ep.validate_plugins();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("never records"), "{errs:?}");
    }

    #[test]
    fn no_dye_produces_no_error() {
        let entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
        let ep = with_request_plugins(vec![entry]);
        assert!(ep.validate_plugins().is_empty());
    }

    #[test]
    fn set_clobber_collision_within_one_array_detected_by_validate_plugins() {
        // RegionRoute declares Success + Fallback capabilities, keeping the
        // per-entry capability check out of the way so the resource-level
        // clobber check is isolated.
        let mut success_entry = RequestFilterEntry::new(EdgionPlugin::RegionRoute(Default::default()));
        success_entry.dye = Some(dye_response("X-Dye", &[DyeOutcome::Success]));

        let mut fallback_entry = RequestFilterEntry::new(EdgionPlugin::RegionRoute(Default::default()));
        fallback_entry.dye = Some(dye_response("x-dye", &[DyeOutcome::Fallback]));

        let ep = with_request_plugins(vec![success_entry, fallback_entry]);
        let errs = ep.validate_plugins();
        assert!(
            errs.iter().any(|e| e.contains("dye: response header")),
            "expected a set-clobber error, got: {errs:?}"
        );
    }

    #[test]
    fn set_clobber_collision_across_stage_arrays_detected_by_validate_plugins() {
        // Success rule lives in requestPlugins, the colliding failure rule in
        // upstreamResponseFilterPlugins — the clobber check runs once per
        // resource over all four stage arrays chained, not per-array.
        let mut success_entry = RequestFilterEntry::new(EdgionPlugin::RegionRoute(Default::default()));
        success_entry.dye = Some(dye_response("X-Dye", &[DyeOutcome::Success]));

        let mut fallback_entry =
            UpstreamResponseFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(header_filter()));
        fallback_entry.dye = Some(dye_response("x-dye", &[DyeOutcome::FailOpen]));

        let ep = EdgionPlugins {
            metadata: Default::default(),
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![success_entry]),
                upstream_response_filter_plugins: Some(vec![fallback_entry]),
                ..Default::default()
            },
            status: None,
        };
        let errs = ep.validate_plugins();
        assert!(
            errs.iter().any(|e| e.contains("dye: response header")),
            "expected a cross-array set-clobber error, got: {errs:?}"
        );
    }

    #[test]
    fn set_clobber_ignores_disabled_entries() {
        // A disabled entry's dye is dormant — must not trip the resource-level
        // clobber check, matching the disabled-entry-is-inert rule used
        // throughout `validate_plugins`.
        let mut success_entry = RequestFilterEntry::new(EdgionPlugin::RegionRoute(Default::default()));
        success_entry.dye = Some(dye_response("X-Dye", &[DyeOutcome::Success]));

        let mut fallback_entry = RequestFilterEntry::with_enable(EdgionPlugin::RegionRoute(Default::default()), false);
        fallback_entry.dye = Some(dye_response("x-dye", &[DyeOutcome::Fallback]));

        let ep = with_request_plugins(vec![success_entry, fallback_entry]);
        let errs = ep.validate_plugins();
        assert!(
            !errs.iter().any(|e| e.contains("dye: response header")),
            "disabled entry's dye must not trigger a clobber error, got: {errs:?}"
        );
    }
}
