//! Shared outbound-request templating types.
//!
//! Defines the operator-facing request spec (`OutboundRequestSpec`) used by both
//! the webhook plugin and (later) the forward_auth plugin, plus all supporting
//! sub-types: `ListFields`, `PathSpec`, `MethodSpec`, `OriginField`, `CustomField`,
//! `CustomSource`, and `PresencePolicy`.

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
        None
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

    /// Absent field causes the call to fail (respects allowDegradation).
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
}
