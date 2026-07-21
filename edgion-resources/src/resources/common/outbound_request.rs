//! Shared outbound-request templating types.
//!
//! Defines the operator-facing request spec (`OutboundRequestSpec`) used by both
//! the webhook plugin and (later) the forward_auth plugin, plus all supporting
//! sub-types: `ListFields`, `PathSpec`, `MethodSpec`, `OriginField`, `CustomField`,
//! `CustomSource`, `PresencePolicy`, and `BodySpec`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ============================================================
// OutboundRequestSpec — outbound request specification
// ============================================================

/// Outbound request specification.
///
/// Declares which path, method, args, headers, and cookies are forwarded to the
/// backend. Omit path/method to reuse the frozen incoming values. Omit
/// args/headers/cookies blocks to forward nothing of that kind (empty allow-list).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutboundRequestSpec {
    /// Outbound path specification. Omit to use frozen incoming path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathSpec>,

    /// Outbound HTTP method. Omit to use frozen incoming method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<MethodSpec>,

    /// Query arguments allow-list (origin + custom).
    #[serde(default)]
    pub args: ListFields,

    /// HTTP headers allow-list (origin + custom).
    #[serde(default)]
    pub headers: ListFields,

    /// Cookies allow-list (origin + custom).
    #[serde(default)]
    pub cookies: ListFields,

    /// Outbound request body. Omit to send no body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodySpec>,
}

/// Canary substituted for every variable by the config-time JSON lint.
/// Non-numeric on purpose: an unquoted placeholder renders as a bare
/// identifier, which fails JSON parsing.
const BODY_LINT_CANARY: &str = "__edgion_canary__";

/// Render `tpl` with every variable replaced by [`BODY_LINT_CANARY`].
fn render_with_canary(tpl: &crate::resources::edgion_plugins::plugin_configs::CompiledTemplate) -> String {
    use crate::resources::edgion_plugins::plugin_configs::TemplateSegment;
    let mut out = String::with_capacity(tpl.hint_len());
    for seg in tpl.segments() {
        match seg {
            TemplateSegment::Literal(s) => out.push_str(s),
            TemplateSegment::Variable { .. } => out.push_str(BODY_LINT_CANARY),
        }
    }
    out
}

impl OutboundRequestSpec {
    /// Validate semantic constraints on this spec and return the first error found.
    ///
    /// Currently enforced:
    /// - `args.forwardAll` and `cookies.forwardAll` are not supported; only
    ///   `headers.forwardAll` is implemented. Returning an error here prevents
    ///   silently no-op configurations that would otherwise confuse operators.
    pub fn validation_error(&self) -> Option<String> {
        if self.args.forward_all || self.cookies.forward_all {
            return Some("forwardAll is only supported on headers; remove it from args/cookies".to_string());
        }
        if let Some(err) = self.body_validation_error() {
            return Some(err);
        }
        None
    }

    /// Validate the `body` block: Content-Type conflicts, template compile
    /// errors, and the canary-render JSON lint. Shared by webhook and
    /// forward_auth via `validation_error`.
    fn body_validation_error(&self) -> Option<String> {
        use crate::resources::edgion_plugins::plugin_configs::CompiledTemplate;
        let body = self.body.as_ref()?;

        // json/form derive their Content-Type; an explicit header conflicts.
        if matches!(body, BodySpec::Json { .. } | BodySpec::Form { .. }) {
            let explicit_ct = self
                .headers
                .origin
                .iter()
                .map(|f| f.name.as_str())
                .chain(self.headers.custom.iter().map(|f| f.name.as_str()))
                .any(|n| n.eq_ignore_ascii_case("content-type"));
            if explicit_ct {
                return Some(
                    "body type json/form auto-injects Content-Type; remove the explicit \
                     Content-Type header (use body type raw to control it yourself)"
                        .to_string(),
                );
            }
        }

        match body {
            BodySpec::Json { template } => match CompiledTemplate::compile(template) {
                Ok(tpl) => {
                    let rendered = render_with_canary(&tpl);
                    if let Err(e) = serde_json::from_str::<serde_json::Value>(&rendered) {
                        return Some(format!(
                            "body.template does not render valid JSON (every variable must sit \
                             inside a JSON string; use body type raw for non-string payloads): {e}"
                        ));
                    }
                    None
                }
                Err(e) => Some(format!("body.template: {e}")),
            },
            BodySpec::Form { fields } => {
                for f in fields {
                    if let CustomSource::Template(t) = &f.source {
                        if let Err(e) = CompiledTemplate::compile(t) {
                            return Some(format!("body.fields[{}]: {e}", f.name));
                        }
                    }
                }
                None
            }
            BodySpec::Raw { template } => CompiledTemplate::compile(template)
                .err()
                .map(|e| format!("body.template: {e}")),
        }
    }

    /// Collect the raw var_names of every `${secretRef:...}` referenced across
    /// the outbound request templates (path, method, args/headers/cookies
    /// custom). Best-effort: templates that fail to compile contribute nothing
    /// (the compile error is surfaced separately by the caller's validation).
    pub fn collect_secret_ref_names(&self) -> Vec<String> {
        use crate::resources::edgion_plugins::plugin_configs::{secret_ref_var_names, CompiledTemplate};
        let mut out = Vec::new();
        let mut scan = |t: &str| {
            if let Ok(c) = CompiledTemplate::compile(t) {
                out.extend(secret_ref_var_names(&c));
            }
        };
        if let Some(p) = &self.path {
            scan(&p.template);
        }
        if let Some(m) = &self.method {
            scan(&m.template);
        }
        for group in [&self.args, &self.headers, &self.cookies] {
            for f in &group.custom {
                if let CustomSource::Template(t) = &f.source {
                    scan(t);
                }
            }
        }
        match &self.body {
            Some(BodySpec::Json { template }) | Some(BodySpec::Raw { template }) => scan(template),
            Some(BodySpec::Form { fields }) => {
                for f in fields {
                    if let CustomSource::Template(t) = &f.source {
                        scan(t);
                    }
                }
            }
            None => {}
        }
        out
    }
}

// ============================================================
// PathSpec
// ============================================================

/// Path specification for outbound requests.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathSpec {
    /// Path template: literal string or ${type:name} interpolation.
    pub template: String,

    /// If true, consumer may pass pathOverride at call time.
    #[serde(default)]
    pub allow_override: bool,
}

// ============================================================
// MethodSpec
// ============================================================

/// HTTP method specification for outbound requests.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MethodSpec {
    /// Method template: literal (POST, GET, etc.) or ${type:name} interpolation.
    pub template: String,
}

// ============================================================
// ListFields
// ============================================================

/// Allow-listed fields (args/headers/cookies) split into origin and custom sources.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListFields {
    /// Forward every field present on the incoming request (headers: minus
    /// hop-by-hop / proxy-managed). Default false (webhook). forward_auth sets true.
    #[serde(default)]
    pub forward_all: bool,

    /// Fields forwarded from the incoming request (frozen at entry time).
    #[serde(default)]
    pub origin: Vec<OriginField>,

    /// Fields explicitly constructed by the operator.
    #[serde(default)]
    pub custom: Vec<CustomField>,
}

// ============================================================
// OriginField
// ============================================================

/// An origin field: forwarded from the incoming request with optional presence control.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OriginField {
    /// Field name in both incoming and outbound request.
    pub name: String,

    /// Behavior when the field is absent from the incoming request.
    #[serde(default)]
    pub presence: PresencePolicy,
}

// ============================================================
// PresencePolicy
// ============================================================

/// Presence policy for origin fields.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PresencePolicy {
    /// Absent field is silently omitted (default).
    SkipIfMissing,

    /// Absent field causes the call to fail. In forward_auth this surfaces as
    /// fail-close (`statusOnError`); for the webhook connector `key_get`
    /// returns `None` and the consuming plugin's fallback decides what happens.
    Required,

    /// Absent field triggers PluginLog alert but request continues.
    WarnIfMissing,
}

impl Default for PresencePolicy {
    fn default() -> Self {
        Self::SkipIfMissing
    }
}

// ============================================================
// CustomField / CustomSource
// ============================================================

/// A custom field injected into the outbound request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomField {
    /// Field name in the outbound request.
    pub name: String,

    /// Source: exactly one of value or template.
    #[serde(flatten)]
    pub source: CustomSource,
}

/// Source for a custom field: literal or template (which may embed
/// `${secretRef:ns/name:key}` for Secret injection).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CustomSource {
    /// Verbatim literal string.
    Value(String),

    /// Template interpolated at call time. May contain `${secretRef:ns/name:key}`
    /// to inject a controller-resolved Kubernetes Secret value.
    Template(String),
}

// ============================================================
// BodySpec
// ============================================================

/// Outbound request body specification.
///
/// `json` and `raw` carry a `${type:name}` template; `form` carries structured
/// fields so each value can be percent-encoded correctly. Design:
/// docs/superpowers/specs/2026-07-13-webhook-dynamic-body-design.md.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum BodySpec {
    /// JSON payload template. Variable values are JSON-string-escaped at eval
    /// time, so every variable renders as (part of) a JSON string; the
    /// config-time canary lint rejects templates whose skeleton is not valid
    /// JSON, including unquoted placeholders. Content-Type: application/json
    /// is auto-injected.
    Json {
        /// Literal JSON with `${type:name}` interpolation.
        template: String,
    },
    /// Structured form fields, percent-encoded as
    /// application/x-www-form-urlencoded (auto-injected Content-Type).
    Form {
        /// Form fields (literal `value` or `template` source).
        fields: Vec<CustomField>,
    },
    /// Verbatim payload after interpolation. No Content-Type is injected; the
    /// operator supplies one via `headers.custom` when needed.
    Raw {
        /// Payload template sent as-is after interpolation.
        template: String,
    },
}

// ============================================================
// Tests (written first — TDD)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_fields_forward_all_defaults_false() {
        let lf: ListFields = serde_yaml::from_str("origin: []\ncustom: []").unwrap();
        assert!(!lf.forward_all);
    }

    #[test]
    fn list_fields_parses_forward_all_true() {
        let lf: ListFields = serde_yaml::from_str("forwardAll: true").unwrap();
        assert!(lf.forward_all);
        assert!(lf.origin.is_empty());
    }

    /// C1: args.forwardAll must be rejected at the schema-validation layer.
    #[test]
    fn args_forward_all_is_rejected() {
        let spec = OutboundRequestSpec {
            args: ListFields {
                forward_all: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let err = spec
            .validation_error()
            .expect("args.forwardAll must produce a validation error");
        assert!(
            err.contains("forwardAll"),
            "Error message should mention forwardAll; got: {err}"
        );
    }

    /// C1: cookies.forwardAll must be rejected at the schema-validation layer.
    #[test]
    fn cookies_forward_all_is_rejected() {
        let spec = OutboundRequestSpec {
            cookies: ListFields {
                forward_all: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let err = spec
            .validation_error()
            .expect("cookies.forwardAll must produce a validation error");
        assert!(
            err.contains("forwardAll"),
            "Error message should mention forwardAll; got: {err}"
        );
    }

    /// Sanity: headers.forwardAll passes validation (it is the supported case).
    #[test]
    fn headers_forward_all_passes_validation() {
        let spec = OutboundRequestSpec {
            headers: ListFields {
                forward_all: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            spec.validation_error().is_none(),
            "headers.forwardAll should pass validation"
        );
    }

    #[test]
    fn body_spec_parses_json_type() {
        let spec: OutboundRequestSpec =
            serde_yaml::from_str("body:\n  type: json\n  template: '{\"userId\": \"${ctx:userId}\"}'").unwrap();
        match spec.body {
            Some(BodySpec::Json { ref template }) => {
                assert!(template.contains("${ctx:userId}"));
            }
            other => panic!("expected Json body, got {other:?}"),
        }
    }

    #[test]
    fn body_spec_parses_form_fields() {
        let yaml = r#"
body:
  type: form
  fields:
    - name: userId
      template: "${ctx:userId}"
    - name: source
      value: edgion
"#;
        let spec: OutboundRequestSpec = serde_yaml::from_str(yaml).unwrap();
        match spec.body {
            Some(BodySpec::Form { ref fields }) => {
                assert_eq!(fields.len(), 2);
                assert!(matches!(fields[1].source, CustomSource::Value(_)));
            }
            other => panic!("expected Form body, got {other:?}"),
        }
    }

    #[test]
    fn body_spec_parses_raw_type() {
        let spec: OutboundRequestSpec =
            serde_yaml::from_str("body:\n  type: raw\n  template: 'plain ${ctx:x}'").unwrap();
        assert!(matches!(spec.body, Some(BodySpec::Raw { .. })));
    }

    #[test]
    fn body_spec_rejects_unknown_type() {
        let r: Result<OutboundRequestSpec, _> = serde_yaml::from_str("body:\n  type: xml\n  template: '<a/>'");
        assert!(r.is_err(), "unknown body type must be rejected");
    }

    #[test]
    fn body_spec_rejects_unknown_field() {
        let r: Result<OutboundRequestSpec, _> =
            serde_yaml::from_str("body:\n  type: json\n  template: '{}'\n  extra: nope");
        assert!(r.is_err(), "unknown field on body must be rejected");
    }

    #[test]
    fn body_defaults_to_none() {
        let spec: OutboundRequestSpec = serde_yaml::from_str("args: {}").unwrap();
        assert!(spec.body.is_none());
    }

    fn body_json(template: &str) -> Option<BodySpec> {
        Some(BodySpec::Json {
            template: template.to_string(),
        })
    }

    #[test]
    fn json_body_with_explicit_content_type_custom_rejected() {
        let spec = OutboundRequestSpec {
            body: body_json(r#"{"a": "${ctx:x}"}"#),
            headers: ListFields {
                custom: vec![CustomField {
                    name: "content-type".to_string(),
                    source: CustomSource::Value("text/plain".to_string()),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let err = spec.validation_error().expect("must reject CT conflict");
        assert!(err.contains("Content-Type"), "got: {err}");
    }

    #[test]
    fn form_body_with_explicit_content_type_origin_rejected() {
        let spec = OutboundRequestSpec {
            body: Some(BodySpec::Form { fields: vec![] }),
            headers: ListFields {
                origin: vec![OriginField {
                    name: "Content-Type".to_string(),
                    presence: PresencePolicy::SkipIfMissing,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(spec.validation_error().is_some(), "origin CT must also conflict");
    }

    #[test]
    fn raw_body_with_explicit_content_type_allowed() {
        let spec = OutboundRequestSpec {
            body: Some(BodySpec::Raw {
                template: "hello".to_string(),
            }),
            headers: ListFields {
                custom: vec![CustomField {
                    name: "Content-Type".to_string(),
                    source: CustomSource::Value("text/csv".to_string()),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            spec.validation_error().is_none(),
            "raw + explicit CT is the supported combo"
        );
    }

    #[test]
    fn json_body_canary_lint_accepts_quoted_placeholders() {
        let spec = OutboundRequestSpec {
            body: body_json(r#"{"userId": "${ctx:userId}", "caller": "edgion-gateway"}"#),
            ..Default::default()
        };
        assert!(spec.validation_error().is_none());
    }

    #[test]
    fn json_body_canary_lint_rejects_unquoted_placeholder() {
        let spec = OutboundRequestSpec {
            body: body_json(r#"{"count": ${ctx:n}}"#),
            ..Default::default()
        };
        let err = spec.validation_error().expect("unquoted placeholder must be rejected");
        assert!(err.contains("valid JSON"), "got: {err}");
    }

    #[test]
    fn json_body_canary_lint_rejects_broken_skeleton() {
        let spec = OutboundRequestSpec {
            body: body_json(r#"{"a": "${ctx:x}""#), // missing closing brace
            ..Default::default()
        };
        assert!(spec.validation_error().is_some());
    }

    #[test]
    fn json_body_compile_error_rejected() {
        let spec = OutboundRequestSpec {
            body: body_json(r#"{"a": "${ctx:x"}"#), // unclosed ${
            ..Default::default()
        };
        assert!(spec.validation_error().is_some());
    }

    #[test]
    fn form_body_field_template_compile_error_rejected() {
        let spec = OutboundRequestSpec {
            body: Some(BodySpec::Form {
                fields: vec![CustomField {
                    name: "u".to_string(),
                    source: CustomSource::Template("${ctx:x".to_string()),
                }],
            }),
            ..Default::default()
        };
        assert!(spec.validation_error().is_some());
    }
}
