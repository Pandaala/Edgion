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
use super::edgion_plugin::EdgionPlugin;
use super::EdgionPlugins;
use crate::resources::common::log_labels::LOG_LABEL_MAX_COUNT;

/// Per-variant stage applicability, mirroring the four
/// `PluginRuntime::create_*_from_edgion` matches in `edgion-gateway`. The
/// exhaustive match means a newly added `EdgionPlugin` variant fails to compile
/// until its stages are declared here, keeping the two in lockstep.
#[derive(Clone, Copy)]
struct StageSet {
    request: bool,
    upstream_response_filter: bool,
    upstream_response_body_filter: bool,
    upstream_response: bool,
}

/// Which execution stages a plugin variant is valid for.
///
/// This mirrors `edgion-gateway`'s `create_*_from_edgion` stage gating: a
/// variant that those functions answer with `WrongStage` is `false` here. The
/// `upstream_response` (async) stage currently accepts only `ExtProc`.
fn applicable_stages(plugin: &EdgionPlugin) -> StageSet {
    use EdgionPlugin::*;
    // Tuple order: (request, upstreamResponseFilter, upstreamResponseBodyFilter, upstreamResponse)
    let (request, upstream_response_filter, upstream_response_body_filter, upstream_response) = match plugin {
        // Upstream-response-filter-stage-only plugins.
        ResponseHeaderModifier(_) | DebugAccessLogToHeader(_) | ResponseRewrite(_) => (false, true, false, false),
        // Upstream-response-body-filter-stage-only plugins.
        BandwidthLimit(_) => (false, false, true, false),
        // DSL runs in both the request and the upstream-response-filter stages.
        Dsl(_) => (true, true, false, false),
        // Wasm (proxy-wasm guest) runs in the request, upstream-response-filter,
        // and upstream-response-body-filter (read-only observation) stages,
        // matching its three CRD stage enum entries.
        Wasm(_) => (true, true, true, false),
        // ExtProc runs in the request stage and the async upstream-response stage.
        ExtProc(_) => (true, false, false, true),
        // Everything else is request-stage-only.
        RequestHeaderModifier(_)
        | RequestRedirect(_)
        | UrlRewrite(_)
        | RequestMirror(_)
        | BasicAuth(_)
        | Cors(_)
        | Csrf(_)
        | IpRestriction(_)
        | JwtAuth(_)
        | JweDecrypt(_)
        | HmacAuth(_)
        | HeaderCertAuth(_)
        | KeyAuth(_)
        | LdapAuth(_)
        | Mock(_)
        | ProxyRewrite(_)
        | RequestRestriction(_)
        | RateLimit(_)
        | RateLimitRedis(_)
        | CtxSet(_)
        | RealIp(_)
        | ForwardAuth(_)
        | OpenidConnect(_)
        | DirectEndpoint(_)
        | AllEndpointStatus(_)
        | DynamicInternalUpstream(_)
        | DynamicExternalUpstream(_)
        | FaultInjection(_)
        | RegionRoute(_)
        | TraceContext(_)
        | GlobalAccessControl(_)
        | Canary(_) => (true, false, false, false),
    };
    StageSet {
        request,
        upstream_response_filter,
        upstream_response_body_filter,
        upstream_response,
    }
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

        // Pass 1: condition regex compilation/validation for ENABLED entries,
        // in stage order — mirrors `compile_plugin_conditions`. A disabled entry
        // (`enable: false`) is semantically inert: the gateway never builds or
        // runs it, so its dormant `conditions` must not be able to flip the
        // resource to `Accepted=False`. Skipping here keeps the schema layer and
        // the gateway `compile_plugin_conditions` in lockstep (parity test in
        // `plugin_store.rs`), and aligns HTTP with the EdgionStreamPlugins rule
        // that a parked broken config is not a validation error.
        macro_rules! compile_conditions {
            ($entries:expr, $label:literal) => {
                if let Some(entries) = &$entries {
                    for (i, entry) in entries.iter().enumerate() {
                        if !entry.is_enabled() {
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

        // Pass 2: per-entry config/alias/stage validation for ENABLED entries,
        // in stage order — mirrors the four `add_from_*` paths (minus runtime
        // construction and the DSL deep checks).
        macro_rules! validate_entries {
            ($entries:expr, $label:literal, $stage_word:literal, $stage_field:ident) => {
                if let Some(entries) = &$entries {
                    for (i, entry) in entries.iter().enumerate() {
                        if !entry.is_enabled() {
                            continue;
                        }
                        let name = entry.plugin.type_name();
                        // 1. Per-plugin config validation. On error, skip the
                        //    remaining checks (mirrors the gateway `continue`).
                        if let Some(err) = entry.plugin.validation_error() {
                            errors.push(format!("{}[{}] ({}): {}", $label, i, name, err));
                            continue;
                        }
                        // 2. Optional alias. Recorded but does NOT skip the stage
                        //    check — the gateway only drops the invalid alias.
                        if let Some(err) = entry.validate_alias() {
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

        // At most one TraceContext in the request stage — multiple would each
        // overwrite x-trace-id (last-write-wins), which is confusing.
        if let Some(entries) = &self.spec.request_plugins {
            let count = entries
                .iter()
                .filter(|e| e.is_enabled() && e.plugin.type_name() == "TraceContext")
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
    use crate::resources::edgion_plugins::plugin_configs::DslConfig;
    use crate::resources::edgion_plugins::{EdgionPluginsSpec, RequestFilterEntry};
    use crate::resources::http_route::HTTPHeaderFilter;

    fn header_filter() -> HTTPHeaderFilter {
        HTTPHeaderFilter {
            set: None,
            add: None,
            remove: None,
        }
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

    fn dsl_entry(source: &str) -> RequestFilterEntry {
        RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
            name: "d".to_string(),
            source: Some(source.to_string()),
            ..Default::default()
        }))
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
