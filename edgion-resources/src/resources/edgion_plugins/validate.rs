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

use super::conditions::{Condition, PluginConditions};
use super::edgion_plugin::{BodyStage, EdgionPlugin};
use super::entry::{dye_set_clobber_error, RequestFilterEntry};
use super::{EdgionPlugins, PluginValidation};
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
/// absent from this table is request-stage-only, except the non-executable sentinel.
///
/// This is the single source of truth for stage applicability. It is keyed by
/// [`EdgionPlugin::type_name`] so the CRD drift guard can consume it directly
/// instead of maintaining its own copy — the mirror table that used to live in
/// `edgion-resources/tests/plugin_crd_type_coverage.rs` is gone.
///
/// New supported names default to request-only. Schema-driven tests reject
/// misspelled exception keys; stage eligibility must also stay aligned with
/// the Gateway's explicit constructors. CRD type strings are intentionally open.
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
    // matching the Gateway stage constructors.
    ("Wasm", StageSet::new(true, true, true, false)),
];

/// Which execution stages a plugin type name is valid for.
///
/// This mirrors `edgion-gateway`'s `create_*_from_edgion` stage gating: a
/// variant that those functions answer with `WrongStage` is `false` here. The
/// The async upstream-response stage currently has no directly configurable
/// plugin. ExtProc response processing is owned by its request-stage entry.
pub fn applicable_stages_by_name(type_name: &str) -> StageSet {
    if type_name == "Unsupported" {
        return StageSet::new(false, false, false, false);
    }
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

/// The single message for a misordered `AiGuard`.
///
/// Shared verbatim by the controller admission path and the gateway
/// `compile_plugins` path so the two error sets stay byte-identical, as
/// `validate_plugins_matches_compile_plugins_error_set` requires.
pub const AI_GUARD_AFTER_AI_PROXY_ERROR: &str =
    "requestPlugins: AiGuard must be ordered before AiProxy; a mask published after AiProxy has captured the \
     canonical body never reaches the provider";

/// One protocol conversion owner per resource, with Controller/Gateway parity.
pub fn collect_grpc_transcoding_errors(entries: &[RequestFilterEntry]) -> Vec<String> {
    let count = entries
        .iter()
        .filter(|entry| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::GrpcJsonTranscoder(_)))
        .count();
    let mut errors = Vec::new();
    if count > 1 {
        errors.push("requestPlugins: at most one GrpcJsonTranscoder plugin is allowed".into());
    }
    if count != 0
        && entries.iter().any(|entry| {
            entry.is_enabled()
                && matches!(
                    entry.plugin,
                    EdgionPlugin::AiProxy(_) | EdgionPlugin::AiGuard(_) | EdgionPlugin::Guardrail(_)
                )
        })
    {
        errors.push("requestPlugins: GrpcJsonTranscoder conflicts with AI conversion owners".into());
    }
    errors
}

/// Report an `AiGuard` entry ordered after `AiProxy` in the same resource.
/// Cross-resource ordering still requires the runtime body-generation guard.
pub fn collect_ai_guard_ordering_errors(entries: &[RequestFilterEntry], errors: &mut Vec<String>) {
    let responses = entries.iter().any(|entry| {
        entry.is_enabled()
            && matches!(&entry.plugin, EdgionPlugin::AiProxy(config) if matches!(config.api, super::AiApi::OpenaiResponses | super::AiApi::AnthropicMessages))
    });
    if responses
        && entries.iter().any(|entry| {
            entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::AiGuard(_) | EdgionPlugin::Guardrail(_))
        })
    {
        errors.push("requestPlugins: native Responses/Messages does not support AiGuard or Guardrail".to_string());
    }
    let active = |entry: &RequestFilterEntry| entry.is_enabled();
    // The LAST guard against the FIRST proxy: `[AiGuard, AiProxy, AiGuard]`
    // still has a genuinely misordered third entry.
    let last_guard = entries
        .iter()
        .rposition(|entry| active(entry) && entry.plugin.type_name() == "AiGuard");
    let first_proxy = entries
        .iter()
        .position(|entry| active(entry) && entry.plugin.type_name() == "AiProxy");
    if let (Some(guard), Some(proxy)) = (last_guard, first_proxy) {
        if guard > proxy {
            errors.push(AI_GUARD_AFTER_AI_PROXY_ERROR.to_string());
        }
    }
}

/// Validate active Redis overflow dependencies within one request-stage array.
/// The caller must also apply ordinary entry/condition validation. No reference
/// lookup crosses resource or stage boundaries; inactive consumers are inert.
pub fn collect_rate_limit_dependency_errors(entries: &[RequestFilterEntry], errors: &mut Vec<String>) {
    for (index, entry) in entries.iter().enumerate() {
        if !entry.is_enabled() || !matches!(entry.plugin, EdgionPlugin::RateLimitRedis(_)) {
            continue;
        }
        let Some(run) = entry.conditions.as_ref().and_then(|conditions| conditions.run.as_ref()) else {
            continue;
        };
        for condition in run.iter_conditions() {
            let Condition::RateLimitExceeded(reference) = condition else {
                continue;
            };
            // Syntax errors belong to the common condition validation pass.
            if super::entry::validate_alias_str(&reference.plugin_alias).is_some() {
                continue;
            }
            let mut targets = entries.iter().enumerate().filter(|(_, candidate)| {
                candidate.is_enabled() && candidate.alias.as_deref() == Some(reference.plugin_alias.as_str())
            });
            let target = targets.next();
            let reason = match target {
                None => Some("must reference an active RateLimitLocal in the same requestPlugins array"),
                Some(_) if targets.next().is_some() => Some("must reference a unique active alias"),
                Some((target_index, _)) if target_index >= index => Some("must reference an earlier entry"),
                Some((_, target)) => match &target.plugin {
                    EdgionPlugin::RateLimitLocal(config)
                        if config.on_limit_exceeded == super::plugin_configs::OnLimitExceeded::Continue =>
                    {
                        None
                    }
                    EdgionPlugin::RateLimitLocal(_) => {
                        Some("must reference a RateLimitLocal with onLimitExceeded: Continue")
                    }
                    _ => Some("must reference a RateLimitLocal entry"),
                },
            };
            if let Some(reason) = reason {
                errors.push(format!(
                    "requestPlugins[{index}] conditions: rateLimitExceeded.pluginAlias '{}': {reason}",
                    reference.plugin_alias
                ));
            }
        }
    }
}

/// Compile + validate the regex conditions on a single entry, pushing any
/// errors with the same wording the gateway's `compile_plugin_conditions` uses.
/// `compile()` mutates in place, so we operate on a clone (only the errors are
/// wanted here).
fn collect_condition_errors(
    conds: &PluginConditions,
    label: &str,
    index: usize,
    redis_run: bool,
    errors: &mut Vec<String>,
) {
    let mut conds = conds.clone();
    if let Err(errs) = conds.compile() {
        for e in errs {
            errors.push(format!("{}[{}] conditions regex: {}", label, index, e));
        }
    }
    let validation = if redis_run {
        conds.validate_for_rate_limit_redis()
    } else {
        conds.validate()
    };
    if let Err(errs) = validation {
        for e in errs {
            errors.push(format!("{}[{}] conditions: {}", label, index, e));
        }
    }
}

/// Same as [`collect_condition_errors`] but for a plugin's secondary body gate
/// conditions (`entry.body.conditions`) — mirrors the gateway's
/// `compile_body_entries` wording (`"... body conditions ..."`).
fn collect_dye_condition_errors(dye: &super::dye::DyeSuite, label: &str, index: usize, errors: &mut Vec<String>) {
    for rule in dye.request.iter().chain(dye.response.iter()) {
        if let Some(conds) = rule.conditions.as_ref() {
            let mut conds = conds.clone();
            if let Err(errs) = conds.compile() {
                for e in errs {
                    errors.push(format!("{}[{}] dye conditions regex: {}", label, index, e));
                }
            }
            if let Err(errs) = conds.validate() {
                for e in errs {
                    errors.push(format!("{}[{}] dye conditions: {}", label, index, e));
                }
            }
        }
    }
}

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
    /// This conversion owner is valid only at HTTPRoute rule scope.
    pub fn has_active_grpc_transcoder(&self) -> bool {
        self.spec
            .request_plugins
            .iter()
            .flatten()
            .any(|entry| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::GrpcJsonTranscoder(_)))
    }

    pub fn has_active_graphql_operation_router(&self) -> bool {
        self.spec
            .request_plugins
            .iter()
            .flatten()
            .any(|entry| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::GraphqlOperationRouter(_)))
    }
    pub fn http_route_only_plugin(&self) -> Option<&'static str> {
        self.spec
            .request_plugins
            .iter()
            .flatten()
            .find(|entry| entry.is_enabled() && entry.plugin.requires_http_route_rule_scope())
            .map(|entry| entry.plugin.type_name())
    }

    /// Whether this resource contributes RequestId rules at an active attachment.
    pub fn has_active_request_id(&self) -> bool {
        self.spec
            .request_plugins
            .iter()
            .flatten()
            .any(|entry| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::RequestId(_)))
    }

    /// Early attachments may only depend on parsed headers, method and W3C ID.
    pub fn validate_request_id_early_attachment(&self) -> Vec<String> {
        if let Some(plugin) = self.http_route_only_plugin() {
            return vec![format!("{plugin} requires HTTPRoute rule scope")];
        }
        use super::conditions::Condition;
        use crate::common::KeyGet;
        let mut errors = Vec::new();
        for (index, entry) in self.spec.request_plugins.iter().flatten().enumerate() {
            if !entry.is_enabled() {
                continue;
            }
            let EdgionPlugin::RequestId(config) = &entry.plugin else {
                continue;
            };
            let late_source = config
                .ids
                .iter()
                .any(|rule| !matches!(rule.source, None | Some(KeyGet::Header { .. }) | Some(KeyGet::TraceId)));
            let late_condition = entry
                .conditions
                .iter()
                .flat_map(|conditions| conditions.skip.iter().chain(conditions.run.iter()))
                .flat_map(|set| set.iter_conditions())
                .any(|condition| {
                    let key = match condition {
                        Condition::KeyExist(c) => &c.key,
                        Condition::KeyMatch(c) => &c.key,
                        _ => return true,
                    };
                    !matches!(key, KeyGet::Header { .. } | KeyGet::Method | KeyGet::TraceId)
                });
            if late_source || late_condition || entry.body.is_some() || entry.dye.is_some() {
                errors.push(format!("requestPlugins[{index}]: early RequestId requires header/traceId sources and header/method/traceId conditions, without body or dye wrappers"));
            }
        }
        errors
    }
    /// Collect the preparse validation errors for this resource WITHOUT building
    /// the gateway plugin runtime.
    ///
    /// Mirrors the `preparse_errors` that `edgion-gateway`'s `compile_plugins`
    /// surfaces into the EdgionPlugins K8s status condition, minus DSL VM init
    /// (`Vm::new`), the one failure that cannot be predicted statically. See
    /// the module docs for the full contract.
    pub fn validate_plugins(&self) -> Vec<String> {
        self.validate_plugins_with_waf(true)
    }

    /// Validate all source-local entries before Controller bundle resolution.
    /// The resolved WAF is validated after reference binding.
    pub fn validate_plugins_before_waf_resolution(&self) -> Vec<String> {
        self.validate_plugins_with_waf(false)
    }

    /// Unknown types invalidate the source even when disabled or policy-masked.
    pub fn unsupported_plugin_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        macro_rules! scan {
            ($entries:expr, $label:literal) => {
                for (index, entry) in $entries.iter().flatten().enumerate() {
                    if matches!(entry.plugin, EdgionPlugin::Unsupported) {
                        errors.push(format!("{}[{}]: unsupported plugin type", $label, index));
                    }
                }
            };
        }
        scan!(self.spec.request_plugins, "requestPlugins");
        scan!(
            self.spec.upstream_response_filter_plugins,
            "upstreamResponseFilterPlugins"
        );
        scan!(
            self.spec.upstream_response_body_filter_plugins,
            "upstreamResponseBodyFilterPlugins"
        );
        scan!(self.spec.upstream_response_plugins, "upstreamResponsePlugins");
        errors
    }

    fn validate_plugins_with_waf(&self, include_waf: bool) -> Vec<String> {
        let mut errors = self.unsupported_plugin_errors();

        if let Some(waf) = self.spec.waf.as_ref().filter(|_| include_waf) {
            if let Some(error) = waf.validation_error() {
                errors.push(format!("waf: {error}"));
            }
        }

        // Pass 1: condition regex compilation/validation for active entries,
        // in stage order — mirrors `compile_plugin_conditions`. The Controller
        // validates every enabled source entry. Gateway scope composition masks
        // excluded entries before using the same validation; disabled entries
        // remain inert in either context.
        macro_rules! compile_conditions {
            ($entries:expr, $label:literal) => {
                if let Some(entries) = &$entries {
                    for (i, entry) in entries.iter().enumerate() {
                        if !entry.is_enabled() || matches!(entry.plugin, EdgionPlugin::Unsupported) {
                            continue;
                        }
                        if let Some(conds) = entry.conditions.as_ref() {
                            collect_condition_errors(
                                conds,
                                $label,
                                i,
                                $label == "requestPlugins" && matches!(entry.plugin, EdgionPlugin::RateLimitRedis(_)),
                                &mut errors,
                            );
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
                        if !entry.is_enabled() || matches!(entry.plugin, EdgionPlugin::Unsupported) {
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

        macro_rules! compile_dye_conditions {
            ($entries:expr, $label:literal) => {
                if let Some(entries) = &$entries {
                    for (i, entry) in entries.iter().enumerate() {
                        if !entry.is_enabled() || matches!(entry.plugin, EdgionPlugin::Unsupported) {
                            continue;
                        }
                        if let Some(dye) = entry.dye.as_ref() {
                            collect_dye_condition_errors(dye, $label, i, &mut errors);
                        }
                    }
                }
            };
        }
        compile_dye_conditions!(self.spec.request_plugins, "requestPlugins");
        compile_dye_conditions!(
            self.spec.upstream_response_filter_plugins,
            "upstreamResponseFilterPlugins"
        );
        compile_dye_conditions!(
            self.spec.upstream_response_body_filter_plugins,
            "upstreamResponseBodyFilterPlugins"
        );
        compile_dye_conditions!(self.spec.upstream_response_plugins, "upstreamResponsePlugins");

        // Request-stage entries whose bounded-structured `body` is invalid. The
        // gateway's `add_from_request_filters` records these in its prepass and
        // then skips the entry entirely, so Pass 2 must skip them too — otherwise
        // the two layers disagree on which entries reach the alias / dye / stage
        // checks (and on which entries claim an alias for the uniqueness rule).
        let bounded_structured_invalid: std::collections::HashSet<usize> =
            self.spec
                .request_plugins
                .as_ref()
                .map(|entries| {
                    entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| {
                            entry.is_enabled()
                                && entry.plugin.validation_error().is_none()
                                && matches!(
                                    entry.plugin,
                                    EdgionPlugin::FormJsonTransform(_)
                                        | EdgionPlugin::GrpcJsonTranscoder(_)
                                        | EdgionPlugin::JsonSchemaValidation(_)
                                        | EdgionPlugin::GraphqlGuard(_)
                                        | EdgionPlugin::GraphqlSafelist(_)
                                        | EdgionPlugin::GraphqlRateLimit(_)
                                        | EdgionPlugin::GraphqlOperationRouter(_)
                                        | EdgionPlugin::ProxyRewrite(super::plugin_configs::ProxyRewriteConfig {
                                            json_body: Some(_),
                                            ..
                                        })
                                )
                                && entry.body.as_ref().is_some_and(|body| {
                                    !entry.plugin.structured_body_validation_errors(body).is_empty()
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
                        if !entry.is_enabled() || matches!(entry.plugin, EdgionPlugin::Unsupported) {
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

        // A dynamically acquired credential has one resolved value and one
        // validity deadline for the whole resource. Allowing multiple active
        // LinkSys-backed injectors would make that controller-owned state
        // ambiguous, including when the entries sit in different stage arrays.
        let mut link_sys_credential_injectors = 0usize;
        macro_rules! count_link_sys_credential_injectors {
            ($entries:expr) => {
                if let Some(entries) = &$entries {
                    link_sys_credential_injectors += entries
                        .iter()
                        .filter(|entry| entry.is_enabled())
                        .filter(|entry| {
                            matches!(
                                &entry.plugin,
                                EdgionPlugin::CredentialInjector(config) if config.is_link_sys_ref()
                            )
                        })
                        .count();
                }
            };
        }
        count_link_sys_credential_injectors!(self.spec.request_plugins);
        count_link_sys_credential_injectors!(self.spec.upstream_response_filter_plugins);
        count_link_sys_credential_injectors!(self.spec.upstream_response_body_filter_plugins);
        count_link_sys_credential_injectors!(self.spec.upstream_response_plugins);
        if link_sys_credential_injectors > 1 {
            errors.push("EdgionPlugins may contain at most one enabled LinkSys-backed CredentialInjector".to_string());
        }

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
            .filter(|e| e.is_enabled())
            .map(|e| e.dye.as_ref())
            .chain(
                self.spec
                    .upstream_response_filter_plugins
                    .iter()
                    .flatten()
                    .filter(|e| e.is_enabled())
                    .map(|e| e.dye.as_ref()),
            )
            .chain(
                self.spec
                    .upstream_response_body_filter_plugins
                    .iter()
                    .flatten()
                    .filter(|e| e.is_enabled())
                    .map(|e| e.dye.as_ref()),
            )
            .chain(
                self.spec
                    .upstream_response_plugins
                    .iter()
                    .flatten()
                    .filter(|e| e.is_enabled())
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
                        if !entry.is_enabled() || entry.plugin.validation_error().is_some() {
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
                                | EdgionPlugin::GrpcJsonTranscoder(_)
                                | EdgionPlugin::JsonSchemaValidation(_)
                                | EdgionPlugin::GraphqlGuard(_)
                                | EdgionPlugin::GraphqlSafelist(_)
                                | EdgionPlugin::GraphqlRateLimit(_)
                                | EdgionPlugin::GraphqlOperationRouter(_)
                                | EdgionPlugin::ProxyRewrite(_)
                        ) {
                            for error in entry.plugin.structured_body_validation_errors(body) {
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
                .filter(|entry| entry.is_enabled() && matches!(entry.plugin, EdgionPlugin::JsonSchemaValidation(_)))
                .count();
            if json_schema_count > 1 {
                errors.push(format!(
                    "requestPlugins: at most one JsonSchemaValidation plugin is allowed, found {json_schema_count}"
                ));
            }

            errors.extend(collect_grpc_transcoding_errors(entries));
            collect_ai_guard_ordering_errors(entries, &mut errors);
            collect_rate_limit_dependency_errors(entries, &mut errors);
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
#[path = "../../../tests/unit/resources/edgion_plugins/validate_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/validate_test/disabled_condition_tests.rs"]
mod disabled_condition_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/validate_test/request_id_composition_tests.rs"]
mod request_id_composition_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/validate_test/dye_validate_tests.rs"]
mod dye_validate_tests;
