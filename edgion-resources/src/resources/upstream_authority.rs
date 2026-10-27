//! Dynamic upstream authority: a wildcard pattern a backend declares, plus the
//! per-request template that fills its single wildcard label.
//!
//! Safety here is structural. The pattern's suffix is a compile-time constant that
//! rendering never touches, and the template is admitted only if its literal tail
//! equals that suffix and its dynamic slot contains no literal `.`. Rendering can
//! therefore only ever produce ONE label, which is concatenated onto the fixed
//! suffix — an injected value has nowhere to go. See
//! docs/superpowers/specs/2026-07-14-dynamic-upstream-authority-design.md §5.4.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

use crate::resources::common::request_template::{
    CompiledTemplate, SecretPolicy, TemplateAdmission, TemplateSegment, VariablePolicy,
};

/// Operator-facing config. Attached to a Service through
/// `EdgionBackendTrafficPolicy.spec.upstreamAuthority`.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpstreamAuthorityConfig {
    /// Wildcard pattern the backend serves, e.g. `*.sandbox.internal`.
    /// Exactly one leading `*.` label. This is the allowlist AND the shape.
    pub pattern: String,
    /// How the wildcard label is produced per request, e.g.
    /// `${ctx:tenant}.sandbox.internal`. Its literal tail must equal the
    /// pattern's suffix, and its dynamic slot must hold no literal `.`.
    pub template: String,
    /// Static authority for active health probes, which have no request to
    /// render from. Required whenever an active health check is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check_host: Option<String>,
}

/// `"*.a.b"` -> `".a.b"`. Errors unless the pattern is exactly one leading
/// wildcard label followed by a non-empty suffix whose every label is a
/// valid DNS label (per `validate_label`). The returned suffix is always
/// lowercase, so `CompiledUpstreamAuthority::compile` and
/// `host_matches_pattern` can both compare against it directly, with no
/// separate case-normalization step of their own.
pub fn pattern_suffix(pattern: &str) -> Result<String, String> {
    let rest = pattern
        .strip_prefix("*.")
        .ok_or_else(|| format!("pattern must start with '*.': got '{pattern}'"))?;
    if rest.is_empty() {
        return Err("pattern must have a suffix after '*.'".to_string());
    }
    if rest.contains('*') {
        return Err(format!("pattern may contain only one wildcard label: got '{pattern}'"));
    }
    // The suffix is spliced verbatim into the upstream Host header and TLS
    // SNI, so it must be validated as strictly as the rendered label: every
    // dot-separated label must pass the same character/length/hyphen rules.
    // A trailing dot (or a doubled dot) surfaces here as a trailing/empty
    // label and is rejected the same way.
    let mut last_label = None;
    for label in rest.split('.') {
        validate_label(label).map_err(|e| format!("pattern suffix label '{label}' is invalid: {e}"))?;
        last_label = Some(label);
    }
    // The composed host is `<one rendered label>.<suffix>`. The rendered
    // label is validated to `[a-z0-9-]`, so it can itself be all-digits
    // (e.g. "127"). If the suffix's FINAL label were also all-digits, a
    // pattern like `*.0.0.1` would let a request-controlled label compose
    // to a 4-label all-numeric host such as "127.0.0.1" — a valid IPv4
    // literal that hyper/reqwest's IP-literal fast path dials directly,
    // bypassing the AnchorPinnedResolver (see call.rs / authority.rs). A
    // real DNS TLD is never all-digits, so rejecting an all-numeric final
    // suffix label costs no legitimate pattern while making an IPv4
    // composition impossible: `std::net::Ipv4Addr::from_str` requires
    // exactly four dotted decimal octets, so a non-numeric final label
    // (guaranteed here) can never parse as an IP address regardless of
    // what the other labels are, and it also rejects hex/octal/dword
    // forms outright (it only accepts dotted-quad decimal).
    if let Some(last) = last_label {
        if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
            return Err(
                "pattern's final label must not be all-numeric (would allow composing an IP address)".to_string(),
            );
        }
    }
    Ok(format!(".{}", rest.to_ascii_lowercase()))
}

/// True when `host` is the pattern's suffix preceded by exactly one label.
/// Case-insensitive, per DNS.
pub fn host_matches_pattern(host: &str, pattern: &str) -> bool {
    let Ok(suffix) = pattern_suffix(pattern) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let Some(label) = host.strip_suffix(&suffix) else {
        return false;
    };
    validate_label(label).is_ok()
}

/// Validate a rendered wildcard label and return it lowercase-normalized.
/// Already-lowercase labels borrow the input.
///
/// The character set excludes `.`, `/`, `?`, `#`, `@`, `:` and CR/LF — every
/// character an injected value would need to grow a second label, escape the
/// host, or split the request.
pub fn validate_label(label: &str) -> Result<Cow<'_, str>, &'static str> {
    if label.is_empty() {
        return Err("empty");
    }
    if label.len() > 63 {
        return Err("longer than 63 bytes");
    }
    let mut has_uppercase = false;
    for byte in label.bytes() {
        if !byte.is_ascii_alphanumeric() && byte != b'-' {
            return Err("must contain only [a-z0-9-]");
        }
        has_uppercase |= byte.is_ascii_uppercase();
    }
    if label.starts_with('-') || label.ends_with('-') {
        return Err("must not start or end with '-'");
    }
    Ok(if has_uppercase {
        Cow::Owned(label.to_ascii_lowercase())
    } else {
        Cow::Borrowed(label)
    })
}

/// The compiled form: a template for the label slot ONLY, plus the fixed suffix.
/// Rendering never touches the suffix — that is what makes escape impossible.
#[derive(Debug, Clone)]
pub struct CompiledUpstreamAuthority {
    pub label_template: CompiledTemplate,
    pub suffix: String,
    pub health_check_host: Option<String>,
}

impl CompiledUpstreamAuthority {
    pub fn compile(cfg: &UpstreamAuthorityConfig) -> Result<Self, String> {
        let suffix = pattern_suffix(&cfg.pattern)?;

        // DNS is case-insensitive, so the tail is matched case-insensitively — but the cut
        // is taken from the ORIGINAL template. Variable types are camelCase (`requestPath`,
        // `matchedHttpRoute`) and ctx/query/cookie/secretRef names are case-sensitive, so the
        // template's interior must survive verbatim. Compare the tail as bytes;
        // a successful match to the validated ASCII suffix proves that the cut
        // below is on a UTF-8 boundary in the original template.
        let suffix_start = cfg.template.len().saturating_sub(suffix.len());
        if !cfg.template.as_bytes()[suffix_start..].eq_ignore_ascii_case(suffix.as_bytes()) {
            return Err(format!("template must end with the pattern suffix '{suffix}'"));
        }
        let label_src = &cfg.template[..suffix_start];
        if label_src.is_empty() {
            return Err("template must have a label slot before the pattern suffix".to_string());
        }

        let label_template =
            CompiledTemplate::compile(label_src).map_err(|e| format!("template is not compilable: {e}"))?;

        (TemplateAdmission {
            variables: VariablePolicy::Trusted,
            secrets: SecretPolicy::Deny,
        })
        .validate(&label_template)
        .map_err(|error| format!("template {error}"))?;

        // A literal '.' anywhere in the label slot would yield two labels;
        // that is the security-relevant case and gets its own message. Any
        // other character outside the rendered label's allowed set would
        // make every request to this backend fail closed at runtime (the
        // literal fragment can never pass `validate_label`), so reject that
        // too, with a separate, more general message.
        for seg in label_template.segments() {
            if let TemplateSegment::Literal(lit) = seg {
                if lit.contains('.') {
                    return Err("template's label slot must not contain a literal '.'".to_string());
                }
                if !lit.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    return Err(format!(
                        "template's label slot contains a literal fragment '{lit}' with a character \
                         outside [a-z0-9-]; it can never pass validation at request time"
                    ));
                }
            }
        }
        // A fully-literal slot is legal but pointless; a slot with no variable at
        // all means the authority is static, which this feature is not for.
        if !label_template
            .segments()
            .iter()
            .any(|s| matches!(s, TemplateSegment::Variable { .. }))
        {
            return Err("template's label slot must contain at least one ${...} variable".to_string());
        }
        // A rendered label can never start or end with '-' (`validate_label` rejects it on
        // every request), so a template whose first or last segment is a literal starting or
        // ending with '-' is statically doomed to fail-closed on every request. Admission
        // should prove a render can succeed, not just that its charset is legal. An interior
        // '-' remains legal.
        if let Some(TemplateSegment::Literal(lit)) = label_template.segments().first() {
            if lit.starts_with('-') {
                return Err(
                    "template's label slot must not start with a literal '-': the rendered label \
                     would fail validation on every request"
                        .to_string(),
                );
            }
        }
        if let Some(TemplateSegment::Literal(lit)) = label_template.segments().last() {
            if lit.ends_with('-') {
                return Err(
                    "template's label slot must not end with a literal '-': the rendered label \
                     would fail validation on every request"
                        .to_string(),
                );
            }
        }

        Ok(Self {
            label_template,
            suffix,
            health_check_host: cfg.health_check_host.clone(),
        })
    }

    /// `<label>` + the fixed suffix. `label` must already be `validate_label`-ed.
    pub fn compose(&self, mut label: String) -> String {
        label.push_str(&self.suffix);
        label
    }
}

/// Admission-time validation. `has_active_health_check` comes from the sibling
/// `spec.healthCheck.active` on the same policy.
pub fn validate_upstream_authority(cfg: &UpstreamAuthorityConfig, has_active_health_check: bool) -> Vec<String> {
    let mut errors = Vec::new();

    if let Err(e) = CompiledUpstreamAuthority::compile(cfg) {
        errors.push(format!("upstreamAuthority: {e}"));
        // Every rule below depends on a well-formed pattern; stop here.
        return errors;
    }

    match (&cfg.health_check_host, has_active_health_check) {
        (None, true) => errors.push(
            "upstreamAuthority.healthCheckHost is required when healthCheck.active is set: an active probe \
             has no request to render the authority from, and a wildcard certificate does not match its own \
             parent domain"
                .to_string(),
        ),
        (Some(h), _) if !host_matches_pattern(h, &cfg.pattern) => errors.push(format!(
            "upstreamAuthority.healthCheckHost '{h}' is outside pattern '{}'",
            cfg.pattern
        )),
        _ => {}
    }

    errors
}

#[cfg(test)]
#[path = "../../tests/unit/resources/upstream_authority_test.rs"]
mod tests;
