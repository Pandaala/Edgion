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

/// Private authored positions shared by admission and owning reference traversal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum RequestTemplatePosition {
    Path,
    Method,
    ArgsCustom(usize),
    HeadersCustom(usize),
    CookiesCustom(usize),
    BodyJson,
    BodyRaw,
    BodyForm(usize),
}

/// Canary substituted for every variable by the config-time JSON lint.
/// Non-numeric on purpose: an unquoted placeholder renders as a bare
/// identifier, which fails JSON parsing.
const BODY_LINT_CANARY: &str = "__edgion_canary__";

/// Render `tpl` with every variable replaced by [`BODY_LINT_CANARY`].
fn render_with_canary(tpl: &crate::resources::common::request_template::CompiledTemplate) -> String {
    use crate::resources::common::request_template::TemplateSegment;
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
        let invalid_header = self
            .headers
            .origin
            .iter()
            .enumerate()
            .map(|(index, field)| ("origin", index, field.name.as_str()))
            .chain(
                self.headers
                    .custom
                    .iter()
                    .enumerate()
                    .map(|(index, field)| ("custom", index, field.name.as_str())),
            )
            .find(|(_, _, name)| {
                name.len() > crate::gwapi_types::HEADER_NAME_MAX_LEN as usize
                    || !crate::gwapi_types::is_valid_header_token(name)
            });
        if let Some((category, index, _)) = invalid_header {
            return Some(format!(
                "request.headers.{category}[{index}].name must be a valid HTTP header name"
            ));
        }
        let explicit_framing_header = self
            .headers
            .origin
            .iter()
            .map(|field| field.name.as_str())
            .chain(self.headers.custom.iter().map(|field| field.name.as_str()))
            .find(|name| name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("transfer-encoding"));
        if let Some(name) = explicit_framing_header {
            return Some(format!(
                "request.headers must not configure proxy-managed framing header {name}; \
                 the outbound client derives framing from the body actually sent"
            ));
        }
        if let Some(err) = self.body_validation_error() {
            return Some(err);
        }
        let mut error = None;
        self.visit_templates(|template| {
            if error.is_none() {
                use super::request_template::{CompiledTemplate, SecretPolicy, TemplateAdmission, VariablePolicy};
                error = CompiledTemplate::compile(template)
                    .and_then(|compiled| {
                        (TemplateAdmission {
                            variables: VariablePolicy::Request,
                            secrets: SecretPolicy::AllowResolved,
                        })
                        .validate(&compiled)
                        .map_err(|error| error.to_string())
                    })
                    .err();
            }
        });
        error
    }

    /// Validate the `body` block: Content-Type conflicts, template compile
    /// errors, and the canary-render JSON lint. Shared by webhook and
    /// forward_auth via `validation_error`.
    fn body_validation_error(&self) -> Option<String> {
        use crate::resources::common::request_template::CompiledTemplate;
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
        use crate::resources::common::request_template::{secret_ref_var_names, CompiledTemplate};
        let mut out = Vec::new();
        self.visit_templates(|t| {
            if let Ok(c) = CompiledTemplate::compile(t) {
                out.extend(secret_ref_var_names(&c));
            }
        });
        out
    }

    /// Visit every template surface for both admission and Secret dependency discovery.
    fn visit_templates(&self, mut scan: impl FnMut(&str)) {
        self.visit_template_fields(|_, template| scan(template));
    }

    pub(crate) fn visit_template_fields(&self, mut scan: impl FnMut(RequestTemplatePosition, &str)) {
        if let Some(p) = &self.path {
            scan(RequestTemplatePosition::Path, &p.template);
        }
        if let Some(m) = &self.method {
            scan(RequestTemplatePosition::Method, &m.template);
        }
        for (group, position) in [
            (
                &self.args,
                RequestTemplatePosition::ArgsCustom as fn(usize) -> RequestTemplatePosition,
            ),
            (&self.headers, RequestTemplatePosition::HeadersCustom),
            (&self.cookies, RequestTemplatePosition::CookiesCustom),
        ] {
            for (index, f) in group.custom.iter().enumerate() {
                if let CustomSource::Template(t) = &f.source {
                    scan(position(index), t);
                }
            }
        }
        match &self.body {
            Some(BodySpec::Json { template }) => scan(RequestTemplatePosition::BodyJson, template),
            Some(BodySpec::Raw { template }) => scan(RequestTemplatePosition::BodyRaw, template),
            Some(BodySpec::Form { fields }) => {
                for (index, f) in fields.iter().enumerate() {
                    if let CustomSource::Template(t) = &f.source {
                        scan(RequestTemplatePosition::BodyForm(index), t);
                    }
                }
            }
            None => {}
        }
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
#[derive(Default)]
pub enum PresencePolicy {
    /// Absent field is silently omitted (default).
    #[default]
    SkipIfMissing,

    /// Absent field causes the call to fail. In forward_auth this surfaces as
    /// fail-close (`statusOnError`); for the webhook connector `key_get`
    /// returns `None` and the consuming plugin's fallback decides what happens.
    Required,

    /// Absent field triggers PluginLog alert but request continues.
    WarnIfMissing,
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
#[path = "../../../tests/unit/resources/common/outbound_request_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/outbound_request_test/template_admission_tests.rs"]
mod template_admission_tests;
