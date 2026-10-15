//! Render the upstream authority for one request.
//!
//! Only the wildcard LABEL is rendered; the suffix is a compile-time constant
//! carried by `CompiledUpstreamAuthority` and concatenated afterwards. An
//! injected value therefore cannot relocate the host — the tail is not data.
//! Spec: docs/superpowers/specs/2026-07-14-dynamic-upstream-authority-design.md §5.4

use edgion_resources::resources::upstream_authority::{validate_label, CompiledUpstreamAuthority};

use crate::link_sys::template::{render_template_in, RenderPolicy, SecretCapability, TemplateContext};
use crate::plugins::runtime::PluginSession;

/// Why an authority could not be produced. Both are hard failures — never a
/// silent fallback to the static authority, which would reach the wrong vhost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityError {
    /// The template rendered to nothing — almost always a `${ctx:…}` variable that
    /// no plugin set, or one set by a plugin that runs after `upstream_peer`.
    RenderEmpty,
    /// The rendered label is not a valid DNS label.
    InvalidLabel,
}

impl AuthorityError {
    /// Stable metric label.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::RenderEmpty => "render_empty",
            Self::InvalidLabel => "invalid_label",
        }
    }
}

/// Render the wildcard label from the session, validate it, and compose the
/// authority. The suffix is never rendered.
pub fn render_authority(c: &CompiledUpstreamAuthority, session: &dyn PluginSession) -> Result<String, AuthorityError> {
    render_authority_in(c, TemplateContext::Request(session))
}

/// Render the same authority using explicit request or trusted capabilities.
pub fn render_authority_in(
    c: &CompiledUpstreamAuthority,
    context: TemplateContext<'_>,
) -> Result<String, AuthorityError> {
    let label = render_template_in(
        &c.label_template,
        context,
        RenderPolicy {
            max_bytes: Some(63),
            ..RenderPolicy::EMPTY
        },
        SecretCapability::Denied,
    )
    .map_err(|_| AuthorityError::InvalidLabel)?
    .into_string();
    if label.is_empty() {
        return Err(AuthorityError::RenderEmpty);
    }
    let label = validate_label(&label).map_err(|_| AuthorityError::InvalidLabel)?;
    let composed = c.compose(&label);
    // Belt-and-suspenders: admission (`pattern_suffix` in
    // edgion-resources/src/resources/upstream_authority.rs) already rejects
    // any pattern whose final suffix label is all-numeric, which makes an
    // IPv4-literal composition unreachable for a compiled pattern. This check
    // is cheap insurance in case a future change to pattern validation
    // regresses that guarantee — it should never fire in practice.
    if composed.parse::<std::net::IpAddr>().is_ok() {
        return Err(AuthorityError::InvalidLabel);
    }
    Ok(composed)
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/policy/backend_traffic/authority_test.rs"]
mod tests;
