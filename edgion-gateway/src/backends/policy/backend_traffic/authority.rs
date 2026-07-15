//! Render the upstream authority for one request.
//!
//! Only the wildcard LABEL is rendered; the suffix is a compile-time constant
//! carried by `CompiledUpstreamAuthority` and concatenated afterwards. An
//! injected value therefore cannot relocate the host — the tail is not data.
//! Spec: docs/superpowers/specs/2026-07-14-dynamic-upstream-authority-design.md §5.4

use edgion_resources::resources::upstream_authority::{validate_label, CompiledUpstreamAuthority};

use crate::link_sys::template::evaluate_template;
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
    let label = evaluate_template(&c.label_template, session);
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;

    fn compiled(template: &str) -> CompiledUpstreamAuthority {
        CompiledUpstreamAuthority::compile(&UpstreamAuthorityConfig {
            pattern: "*.sandbox.internal".to_string(),
            template: template.to_string(),
            health_check_host: None,
        })
        .expect("compiles")
    }

    /// A `MockPluginSession` whose `get_ctx_var("tenant")` returns `value`.
    fn session_with_tenant(value: &str) -> MockPluginSession {
        let mut s = MockPluginSession::new();
        let value = value.to_string();
        s.expect_get_ctx_var()
            .withf(|n| n == "tenant")
            .return_const(Some(value));
        s
    }

    #[test]
    fn renders_and_composes_a_valid_label() {
        let c = compiled("${ctx:tenant}.sandbox.internal");
        let s = session_with_tenant("tenant-a");
        assert_eq!(render_authority(&c, &s).unwrap(), "tenant-a.sandbox.internal");
    }

    #[test]
    fn lowercases_the_rendered_label() {
        let c = compiled("${ctx:tenant}.sandbox.internal");
        let s = session_with_tenant("Tenant-A");
        assert_eq!(render_authority(&c, &s).unwrap(), "tenant-a.sandbox.internal");
    }

    #[test]
    fn unset_ctx_var_is_render_empty_not_an_empty_label() {
        let c = compiled("${ctx:tenant}.sandbox.internal");
        let mut s = MockPluginSession::new();
        s.expect_get_ctx_var().withf(|n| n == "tenant").return_const(None);
        assert!(matches!(render_authority(&c, &s), Err(AuthorityError::RenderEmpty)));
    }

    #[test]
    fn injected_separators_are_rejected_not_escaped() {
        let c = compiled("${ctx:tenant}.sandbox.internal");
        for bad in ["evil.com", "a/b", "a@evil.com", "a:8080", "a\r\nX: y"] {
            let s = session_with_tenant(bad);
            assert!(
                matches!(render_authority(&c, &s), Err(AuthorityError::InvalidLabel)),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn render_authority_rejects_a_composed_ip_literal() {
        // Admission (`pattern_suffix`) already rejects any pattern whose final
        // suffix label is all-numeric, so a compiled pattern can never reach
        // this state in practice. This test pins the layer-2 belt-and-suspenders
        // guard directly by constructing a `CompiledUpstreamAuthority` whose
        // suffix bypasses `compile()`'s validation, so a request-controlled
        // label of "127" composes to the IPv4 literal "127.0.0.1".
        use edgion_resources::resources::edgion_plugins::plugin_configs::CompiledTemplate;

        let c = CompiledUpstreamAuthority {
            label_template: CompiledTemplate::compile("${ctx:tenant}").expect("compiles"),
            suffix: ".0.0.1".to_string(),
            health_check_host: None,
        };
        let s = session_with_tenant("127");
        assert!(
            matches!(render_authority(&c, &s), Err(AuthorityError::InvalidLabel)),
            "a composed IPv4 literal must be rejected even if it bypassed admission"
        );
    }
}
