//! Compiled-template evaluation engine.
//!
//! Resolves a [`CompiledTemplate`] against a live [`PluginSession`] by walking
//! its pre-parsed segments. Because the segment list is fixed at config-load
//! time, values pulled from the session (headers, query, ctx, ...) are emitted
//! verbatim and never re-parsed as template directives -- this eliminates the
//! template injection class present in naive string-substitution approaches.

use crate::plugins::runtime::PluginSession;
use edgion_resources::resources::edgion_plugins::{CompiledTemplate, TemplateSegment, TemplateVarType};

/// Evaluate a pre-compiled template by resolving each segment.
///
/// Because the template structure is fixed at config-load time, user-supplied
/// values that happen to contain `${…}` are emitted verbatim -- they are
/// never re-parsed as template directives. This eliminates the template
/// injection vulnerability present in naive `resolve_template` approaches.
pub fn evaluate_template(compiled: &CompiledTemplate, session: &dyn PluginSession) -> String {
    let mut result = String::with_capacity(compiled.hint_len());
    for segment in compiled.segments() {
        match segment {
            TemplateSegment::Literal(lit) => result.push_str(lit),
            TemplateSegment::Variable { var_type, var_name } => {
                let value = resolve_var(var_type, var_name.as_deref(), session);
                result.push_str(&value);
            }
        }
    }
    result
}

/// Why bounded template evaluation could not produce a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateEvaluationError {
    /// A referenced request value was not present.
    MissingVariable,
    /// The rendered UTF-8 value would exceed the caller's byte budget.
    TooLarge,
}

/// Evaluate a template without ever growing the output past `max_bytes`.
///
/// Unlike the legacy convenience evaluator, this API treats a missing source
/// as an error. Consumers that mutate request bodies need that distinction so
/// an absent value cannot silently become an empty JSON string.
pub fn evaluate_template_bounded(
    compiled: &CompiledTemplate,
    session: &dyn PluginSession,
    max_bytes: usize,
) -> Result<String, TemplateEvaluationError> {
    let mut result = String::with_capacity(compiled.hint_len().min(max_bytes));
    for segment in compiled.segments() {
        let value = match segment {
            TemplateSegment::Literal(lit) => std::borrow::Cow::Borrowed(lit.as_str()),
            TemplateSegment::Variable { var_type, var_name } => {
                std::borrow::Cow::Owned(resolve_var_present(var_type, var_name.as_deref(), session)?)
            }
        };
        if result
            .len()
            .checked_add(value.len())
            .is_none_or(|new_len| new_len > max_bytes)
        {
            return Err(TemplateEvaluationError::TooLarge);
        }
        result.push_str(&value);
    }
    Ok(result)
}

fn resolve_var_present(
    var_type: &TemplateVarType,
    var_name: Option<&str>,
    session: &dyn PluginSession,
) -> Result<String, TemplateEvaluationError> {
    let value = match var_type {
        TemplateVarType::Header => var_name.and_then(|n| session.header_value(n)),
        TemplateVarType::Query => var_name.and_then(|n| session.get_query_param(n)),
        TemplateVarType::Cookie => var_name.and_then(|n| session.get_cookie(n)),
        TemplateVarType::Ctx => var_name.and_then(|n| session.get_ctx_var(n)),
        TemplateVarType::Path => Some(session.get_path().to_string()),
        TemplateVarType::Uri => Some(session.get_uri()),
        TemplateVarType::RequestPath => Some(session.get_request_path().to_string()),
        TemplateVarType::RequestUri => Some(session.get_request_uri()),
        TemplateVarType::Method => Some(session.get_method().to_string()),
        TemplateVarType::ClientIp => Some(session.remote_addr().to_string()),
        TemplateVarType::MatchedHttpRoute => var_name
            .and_then(|n| session.matched_route_meta(crate::plugins::runtime::traits::session::RouteProto::Http, n)),
        TemplateVarType::MatchedGrpcRoute => var_name
            .and_then(|n| session.matched_route_meta(crate::plugins::runtime::traits::session::RouteProto::Grpc, n)),
        TemplateVarType::SecretRef => None,
    };
    value.ok_or(TemplateEvaluationError::MissingVariable)
}

/// Resolve a single variable by type and optional name.
///
/// Returns the resolved value, or an empty string when the named source is
/// absent from the session.
pub(crate) fn resolve_var(var_type: &TemplateVarType, var_name: Option<&str>, session: &dyn PluginSession) -> String {
    match var_type {
        TemplateVarType::Header => var_name.and_then(|n| session.header_value(n)).unwrap_or_default(),
        TemplateVarType::Query => var_name.and_then(|n| session.get_query_param(n)).unwrap_or_default(),
        TemplateVarType::Cookie => var_name.and_then(|n| session.get_cookie(n)).unwrap_or_default(),
        TemplateVarType::Ctx => var_name.and_then(|n| session.get_ctx_var(n)).unwrap_or_default(),
        TemplateVarType::Path => session.get_path().to_string(),
        TemplateVarType::Uri => session.get_uri(),
        TemplateVarType::RequestPath => session.get_request_path().to_string(),
        TemplateVarType::RequestUri => session.get_request_uri(),
        TemplateVarType::Method => session.get_method().to_string(),
        TemplateVarType::ClientIp => session.remote_addr().to_string(),
        TemplateVarType::MatchedHttpRoute => var_name
            .and_then(|n| session.matched_route_meta(crate::plugins::runtime::traits::session::RouteProto::Http, n))
            .unwrap_or_default(),
        TemplateVarType::MatchedGrpcRoute => var_name
            .and_then(|n| session.matched_route_meta(crate::plugins::runtime::traits::session::RouteProto::Grpc, n))
            .unwrap_or_default(),
        // secretRef is webhook-only; the shared engine (ctx_set) has no secret
        // source, so it resolves to empty. This is a B2-leak backstop — ctx_set
        // is also rejected at validation, but this guarantees no value can ever
        // be surfaced here even if that guard were bypassed.
        TemplateVarType::SecretRef => String::new(),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    /// Compile a raw template string into a `CompiledTemplate`.
    fn compile(raw: &str) -> CompiledTemplate {
        CompiledTemplate::compile(raw).expect("template should compile")
    }

    #[test]
    fn test_evaluate_literal_template() {
        let compiled = compile("hello world");
        let session = MockPluginSession::new();
        assert_eq!(evaluate_template(&compiled, &session), "hello world");
    }

    #[test]
    fn test_evaluate_header_variable() {
        let compiled = compile("${header:X-Tenant}");
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "X-Tenant")
            .return_const(Some("acme".to_string()));
        assert_eq!(evaluate_template(&compiled, &session), "acme");
    }

    #[test]
    fn test_evaluate_ctx_variable() {
        let compiled = compile("${ctx:tier}");
        let mut session = MockPluginSession::new();
        session
            .expect_get_ctx_var()
            .withf(|n| n == "tier")
            .return_const(Some("gold".to_string()));
        assert_eq!(evaluate_template(&compiled, &session), "gold");
    }

    #[test]
    fn test_resolve_missing_variable_returns_empty() {
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "X-Absent")
            .return_const(None);
        let value = resolve_var(&TemplateVarType::Header, Some("X-Absent"), &session);
        assert_eq!(value, "");
    }

    #[test]
    fn test_resolve_present_variable() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_query_param()
            .withf(|n| n == "id")
            .return_const(Some("42".to_string()));
        let value = resolve_var(&TemplateVarType::Query, Some("id"), &session);
        assert_eq!(value, "42");
    }

    #[test]
    fn test_multi_variable_template() {
        let compiled = compile("${header:X-User}@${ctx:domain}");
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "X-User")
            .return_const(Some("alice".to_string()));
        session
            .expect_get_ctx_var()
            .withf(|n| n == "domain")
            .return_const(Some("example.com".to_string()));
        assert_eq!(evaluate_template(&compiled, &session), "alice@example.com");
    }

    #[test]
    fn test_resolve_matched_http_route() {
        use crate::plugins::runtime::traits::session::RouteProto;
        let mut session = MockPluginSession::new();
        session
            .expect_matched_route_meta()
            .withf(|p, s| *p == RouteProto::Http && s == "name")
            .return_const(Some("route1".to_string()));
        let v = resolve_var(&TemplateVarType::MatchedHttpRoute, Some("name"), &session);
        assert_eq!(v, "route1");
    }

    #[test]
    fn test_secret_ref_resolves_empty_in_shared_engine() {
        let compiled = compile("x=${secretRef:ns/n:k}");
        let session = MockPluginSession::new();
        // The shared engine (used by ctx_set) has no secret source: secretRef is empty.
        assert_eq!(evaluate_template(&compiled, &session), "x=");
    }

    #[test]
    fn test_template_injection_prevention() {
        // A resolved value that itself contains `${…}` must be emitted verbatim,
        // never re-parsed as a nested directive.
        let compiled = compile("${header:X-Inject}");
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "X-Inject")
            .return_const(Some("${header:Authorization}".to_string()));
        assert_eq!(evaluate_template(&compiled, &session), "${header:Authorization}");
    }

    #[test]
    fn bounded_template_rejects_missing_variable() {
        let compiled = compile("${header:X-Absent}");
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "X-Absent")
            .return_const(None);
        assert_eq!(
            evaluate_template_bounded(&compiled, &session, 64),
            Err(TemplateEvaluationError::MissingVariable)
        );
    }

    #[test]
    fn bounded_template_checks_before_append() {
        let compiled = compile("prefix-${header:X-Value}");
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "X-Value")
            .return_const(Some("value".to_string()));
        assert_eq!(
            evaluate_template_bounded(&compiled, &session, 11),
            Err(TemplateEvaluationError::TooLarge)
        );
        assert_eq!(
            evaluate_template_bounded(&compiled, &session, 12).unwrap(),
            "prefix-value"
        );
    }
}
