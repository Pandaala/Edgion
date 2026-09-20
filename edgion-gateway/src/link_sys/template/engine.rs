//! Canonical request-template runtime: one segment walk, explicit consumer policies.
//!
//! Compiled segments are never reparsed. The output budget bounds rendered UTF-8
//! bytes, not owned source values returned by session accessors or allocator rounding.

use crate::plugins::runtime::{traits::session::RouteProto, PluginSession};
use edgion_resources::resources::common::{
    external_http::ResolvedSecrets,
    request_template::{CompiledTemplate, TemplateSegment, TemplateVarType},
};

#[derive(Clone, Copy, Debug)]
pub enum MissingValuePolicy {
    Empty,
    Error,
}

#[derive(Clone, Copy, Debug)]
pub enum EscapeMode {
    None,
    JsonString,
}

/// Capability borrowed from the prepared outbound client, never a session lookup.
#[derive(Clone, Copy)]
pub enum SecretCapability<'a> {
    Denied,
    Resolved(Option<&'a ResolvedSecrets>),
}

#[derive(Clone, Copy, Debug)]
pub struct RenderPolicy {
    pub missing: MissingValuePolicy,
    pub escape: EscapeMode,
    pub max_bytes: Option<usize>,
}

impl RenderPolicy {
    pub const EMPTY: Self = Self {
        missing: MissingValuePolicy::Empty,
        escape: EscapeMode::None,
        max_bytes: None,
    };
}

/// Rendered bytes and their sensitivity travel together until the destination sink.
pub struct RenderedTemplate {
    value: String,
    sensitive: bool,
}

impl RenderedTemplate {
    pub fn into_string(self) -> String {
        self.value
    }

    /// Consume bytes and their sensitivity (Secrets, marked headers, or cookies).
    pub fn into_parts(self) -> (String, bool) {
        (self.value, self.sensitive)
    }
}

impl std::fmt::Debug for RenderedTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderedTemplate")
            .field("value", &"[redacted]")
            .field("sensitive", &self.sensitive)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateEvaluationError {
    MissingVariable,
    TooLarge,
    SecretNotAllowed,
}

impl std::fmt::Display for TemplateEvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingVariable => "template variable missing",
            Self::TooLarge => "template output too large",
            Self::SecretNotAllowed => "template Secret capability denied",
        })
    }
}

/// One walker for every runtime consumer. Literal segments bypass escaping.
pub fn render_template(
    compiled: &CompiledTemplate,
    session: &dyn PluginSession,
    policy: RenderPolicy,
    secrets: SecretCapability<'_>,
) -> Result<RenderedTemplate, TemplateEvaluationError> {
    let mut out = Output {
        value: String::with_capacity(compiled.hint_len().min(policy.max_bytes.unwrap_or(usize::MAX))),
        max_bytes: policy.max_bytes,
    };
    let mut sensitive = false;
    for segment in compiled.segments() {
        match segment {
            TemplateSegment::Literal(value) => out.append(value)?,
            TemplateSegment::Variable { var_type, var_name } => {
                let resolved = resolve_var(var_type, var_name.as_deref(), session, secrets)?;
                sensitive |= resolved.sensitive;
                match resolved.value {
                    Some(value) => match policy.escape {
                        EscapeMode::None => out.append(&value)?,
                        EscapeMode::JsonString => out.append_json(&value)?,
                    },
                    None if matches!(policy.missing, MissingValuePolicy::Error) => {
                        return Err(TemplateEvaluationError::MissingVariable);
                    }
                    None => {}
                }
            }
        }
    }
    Ok(RenderedTemplate {
        value: out.value,
        sensitive,
    })
}

/// Session-only facade: admission rejects Secrets; invalid artifacts emit no value.
pub fn evaluate_template(compiled: &CompiledTemplate, session: &dyn PluginSession) -> String {
    render_template(compiled, session, RenderPolicy::EMPTY, SecretCapability::Denied)
        .map(RenderedTemplate::into_string)
        .unwrap_or_default()
}

/// ProxyRewrite consumes the result as a JSON string value, without pre-escaping.
pub fn evaluate_template_bounded(
    compiled: &CompiledTemplate,
    session: &dyn PluginSession,
    max_bytes: usize,
) -> Result<String, TemplateEvaluationError> {
    render_template(
        compiled,
        session,
        RenderPolicy {
            missing: MissingValuePolicy::Error,
            escape: EscapeMode::None,
            max_bytes: Some(max_bytes),
        },
        SecretCapability::Denied,
    )
    .map(RenderedTemplate::into_string)
}

struct ResolvedVariable {
    value: Option<String>,
    sensitive: bool,
}

fn resolve_var(
    var_type: &TemplateVarType,
    name: Option<&str>,
    session: &dyn PluginSession,
    secrets: SecretCapability<'_>,
) -> Result<ResolvedVariable, TemplateEvaluationError> {
    // Cookie accessors do not expose per-cookie metadata. Preserve the same
    // conservative protection as reconstructed outbound Cookie headers.
    let mut sensitive = matches!(var_type, TemplateVarType::SecretRef | TemplateVarType::Cookie);
    let value = match var_type {
        TemplateVarType::Header => name
            .and_then(|name| session.request_header_map().get(name))
            .and_then(|value| {
                sensitive = value.is_sensitive();
                value.to_str().ok().map(str::to_string)
            }),
        TemplateVarType::Query => name.and_then(|n| session.get_query_param(n)),
        TemplateVarType::Cookie => name.and_then(|n| session.get_cookie(n)),
        TemplateVarType::Ctx => name.and_then(|n| session.get_ctx_var(n)),
        TemplateVarType::Path => Some(session.get_path().to_string()),
        TemplateVarType::Uri => Some(session.get_uri()),
        TemplateVarType::RequestPath => Some(session.get_request_path().to_string()),
        TemplateVarType::RequestUri => Some(session.get_request_uri()),
        TemplateVarType::Method => Some(session.get_method().to_string()),
        TemplateVarType::RemoteIp => {
            let ip = session.remote_ip();
            (!ip.is_empty()).then(|| ip.to_string())
        }
        TemplateVarType::MatchedHttpRoute => name.and_then(|n| session.matched_route_meta(RouteProto::Http, n)),
        TemplateVarType::MatchedGrpcRoute => name.and_then(|n| session.matched_route_meta(RouteProto::Grpc, n)),
        TemplateVarType::SecretRef => match secrets {
            SecretCapability::Denied => return Err(TemplateEvaluationError::SecretNotAllowed),
            SecretCapability::Resolved(resolved) => name.and_then(|n| resolved?.0.get(n).cloned()),
        },
    };
    Ok(ResolvedVariable { value, sensitive })
}

struct Output {
    value: String,
    max_bytes: Option<usize>,
}

fn checked_output_len(current: usize, extra: usize, cap: Option<usize>) -> Result<usize, TemplateEvaluationError> {
    current
        .checked_add(extra)
        .filter(|len| cap.is_none_or(|cap| *len <= cap))
        .ok_or(TemplateEvaluationError::TooLarge)
}

impl Output {
    fn append(&mut self, value: &str) -> Result<(), TemplateEvaluationError> {
        let next = checked_output_len(self.value.len(), value.len(), self.max_bytes)?;
        if let Some(cap) = self.max_bytes.filter(|_| next > self.value.capacity()) {
            // Preserve amortized growth while capping the requested capacity.
            let capacity = next.max(self.value.capacity().saturating_mul(2)).min(cap);
            self.value.reserve_exact(capacity - self.value.len());
        }
        self.value.push_str(value);
        Ok(())
    }

    fn append_json(&mut self, value: &str) -> Result<(), TemplateEvaluationError> {
        for c in value.chars() {
            match c {
                '"' => self.append("\\\"")?,
                '\\' => self.append("\\\\")?,
                '\n' => self.append("\\n")?,
                '\r' => self.append("\\r")?,
                '\t' => self.append("\\t")?,
                c if (c as u32) < 0x20 => {
                    const CONTROLS: [&str; 32] = [
                        "\\u0000", "\\u0001", "\\u0002", "\\u0003", "\\u0004", "\\u0005", "\\u0006", "\\u0007",
                        "\\u0008", "\\u0009", "\\u000a", "\\u000b", "\\u000c", "\\u000d", "\\u000e", "\\u000f",
                        "\\u0010", "\\u0011", "\\u0012", "\\u0013", "\\u0014", "\\u0015", "\\u0016", "\\u0017",
                        "\\u0018", "\\u0019", "\\u001a", "\\u001b", "\\u001c", "\\u001d", "\\u001e", "\\u001f",
                    ];
                    // Check the complete escape before appending, without a temporary String.
                    self.append(CONTROLS[c as usize])?;
                }
                c => self.append(c.encode_utf8(&mut [0; 4]))?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    /// Compile a raw template string into a `CompiledTemplate`.
    fn compile(raw: &str) -> CompiledTemplate {
        CompiledTemplate::compile(raw).expect("template should compile")
    }

    fn named_session(value: Option<&str>) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        let value = value.map(str::to_string);
        let mut headers = http::HeaderMap::new();
        if let Some(value) = &value {
            headers.insert("x", http::HeaderValue::from_str(value).unwrap());
        }
        session.expect_request_header_map().return_const(headers);
        session.expect_get_query_param().return_const(value.clone());
        session.expect_get_cookie().return_const(value.clone());
        session.expect_get_ctx_var().return_const(value.clone());
        session.expect_matched_route_meta().return_const(value);
        session
    }

    #[test]
    fn named_variables_distinguish_missing_and_present_empty() {
        for variable in [
            "header:x",
            "query:x",
            "cookie:x",
            "ctx:x",
            "matchedHttpRoute:name",
            "matchedGrpcRoute:name",
        ] {
            let tpl = compile(&format!("${{{variable}}}"));
            for value in [Some("value"), Some(""), None] {
                let session = named_session(value);
                assert_eq!(evaluate_template(&tpl, &session), value.unwrap_or_default());
                assert_eq!(
                    evaluate_template_bounded(&tpl, &session, 100),
                    value
                        .map(str::to_string)
                        .ok_or(TemplateEvaluationError::MissingVariable)
                );
            }
        }
    }

    #[test]
    fn intrinsic_variables_preserve_current_and_original_values() {
        let mut session = MockPluginSession::new();
        session.expect_get_path().return_const("/current".to_string());
        session.expect_get_uri().return_const("/current?a=1".to_string());
        session.expect_get_request_path().return_const("/original".to_string());
        session
            .expect_get_request_uri()
            .return_const("/original?b=2".to_string());
        session.expect_get_method().return_const("POST".to_string());
        session.expect_remote_ip().return_const("192.0.2.1".to_string());
        for (name, expected) in [
            ("path", "/current"),
            ("uri", "/current?a=1"),
            ("requestPath", "/original"),
            ("requestUri", "/original?b=2"),
            ("method", "POST"),
            ("remoteIp", "192.0.2.1"),
        ] {
            assert_eq!(
                evaluate_template_bounded(&compile(&format!("${{{name}}}")), &session, 100).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn unavailable_remote_ip_uses_consumer_missing_policy() {
        let mut session = MockPluginSession::new();
        session.expect_remote_ip().return_const(String::new());
        let tpl = compile("${remoteIp}");
        assert_eq!(evaluate_template(&tpl, &session), "");
        assert_eq!(
            evaluate_template_bounded(&tpl, &session, 63),
            Err(TemplateEvaluationError::MissingVariable)
        );
    }

    #[test]
    fn secret_capability_provenance_and_debug_are_explicit() {
        let tpl = compile("Bearer ${secretRef:ns/token:key}");
        let session = MockPluginSession::new();
        let secrets = ResolvedSecrets(std::collections::BTreeMap::from([(
            "ns/token:key".to_string(),
            "sentinel-secret".to_string(),
        )]));
        assert_eq!(
            render_template(&tpl, &session, RenderPolicy::EMPTY, SecretCapability::Denied).unwrap_err(),
            TemplateEvaluationError::SecretNotAllowed
        );
        for map in [Some(&secrets), None] {
            let rendered =
                render_template(&tpl, &session, RenderPolicy::EMPTY, SecretCapability::Resolved(map)).unwrap();
            assert!(!format!("{rendered:?}").contains("sentinel-secret"));
            let (value, secret) = rendered.into_parts();
            assert!(secret);
            assert_eq!(
                value,
                if map.is_some() {
                    "Bearer sentinel-secret"
                } else {
                    "Bearer "
                }
            );
        }
        let strict = RenderPolicy {
            missing: MissingValuePolicy::Error,
            ..RenderPolicy::EMPTY
        };
        assert_eq!(
            render_template(&tpl, &session, strict, SecretCapability::Resolved(None)).unwrap_err(),
            TemplateEvaluationError::MissingVariable
        );
        let empty = ResolvedSecrets(std::collections::BTreeMap::from([(
            "ns/token:key".to_string(),
            String::new(),
        )]));
        assert_eq!(
            render_template(&tpl, &session, strict, SecretCapability::Resolved(Some(&empty)))
                .unwrap()
                .into_parts(),
            ("Bearer ".into(), true)
        );
        assert_eq!(
            render_template(
                &compile("plain"),
                &session,
                RenderPolicy::EMPTY,
                SecretCapability::Denied
            )
            .unwrap()
            .into_parts(),
            ("plain".into(), false)
        );
    }

    #[test]
    fn budget_checks_literals_utf8_aggregate_and_overflow() {
        let session = named_session(Some("é"));
        for (raw, cap, expected) in [
            ("", 0, Some("")),
            ("x", 0, None),
            ("é", 2, Some("é")),
            ("é", 1, None),
            ("${ctx:x}${ctx:x}", 4, Some("éé")),
            ("${ctx:x}${ctx:x}", 3, None),
        ] {
            let result = render_template(
                &compile(raw),
                &session,
                RenderPolicy {
                    max_bytes: Some(cap),
                    ..RenderPolicy::EMPTY
                },
                SecretCapability::Denied,
            )
            .map(RenderedTemplate::into_string);
            assert_eq!(
                result,
                expected.map(str::to_string).ok_or(TemplateEvaluationError::TooLarge)
            );
        }
        assert_eq!(
            checked_output_len(usize::MAX, 1, None),
            Err(TemplateEvaluationError::TooLarge)
        );
        let mut out = Output {
            value: "abc".to_string(),
            max_bytes: Some(4),
        };
        assert_eq!(out.append("de"), Err(TemplateEvaluationError::TooLarge));
        assert_eq!(out.value, "abc");
    }

    #[test]
    fn bounded_output_keeps_amortized_capacity_growth() {
        let mut out = Output {
            value: String::with_capacity(32_000),
            max_bytes: Some(128_000),
        };
        let segment = "a".repeat(128);
        let mut growths = 0;
        for _ in 0..1_000 {
            let before = out.value.capacity();
            out.append(&segment).unwrap();
            growths += usize::from(out.value.capacity() != before);
        }
        assert_eq!(out.value, "a".repeat(128_000));
        assert!(growths <= 2, "unexpected capacity growths: {growths}");
        assert!(out.value.capacity() <= 128_000);
        assert_eq!(out.append("x"), Err(TemplateEvaluationError::TooLarge));

        let mut out = Output {
            value: String::with_capacity(1),
            max_bytes: Some(1_000),
        };
        let mut growths = 0;
        for _ in 0..1_000 {
            let before = out.value.capacity();
            out.append_json("a").unwrap();
            growths += usize::from(out.value.capacity() != before);
        }
        assert!(growths <= 10, "unexpected JSON capacity growths: {growths}");
        assert_eq!(out.value, "a".repeat(1_000));
        assert!(out.value.capacity() <= 1_000);
    }

    #[test]
    fn header_sensitivity_preserves_first_value_and_utf8_filtering() {
        let mut session = MockPluginSession::new();
        let mut headers = http::HeaderMap::new();
        let mut private = http::HeaderValue::from_static("private");
        private.set_sensitive(true);
        headers.append("x-private", private);
        headers.append("x-private", http::HeaderValue::from_static("ignored-second"));
        headers.insert("x-visible", http::HeaderValue::from_static("visible"));
        headers.insert("x-empty", http::HeaderValue::from_static(""));
        headers.insert("x-raw", http::HeaderValue::from_bytes(&[0xff]).unwrap());
        session.expect_request_header_map().return_const(headers);
        for (name, expected, sensitive) in [
            ("x-private", "private", true),
            ("x-visible", "visible", false),
            ("x-empty", "", false),
            ("x-raw", "", false),
            ("x-absent", "", false),
        ] {
            let tpl = compile(&format!("prefix-${{header:{name}}}"));
            let rendered = render_template(&tpl, &session, RenderPolicy::EMPTY, SecretCapability::Denied).unwrap();
            assert_eq!(rendered.into_parts(), (format!("prefix-{expected}"), sensitive));
        }
        assert_eq!(
            evaluate_template_bounded(&compile("${header:x-raw}"), &session, 100),
            Err(TemplateEvaluationError::MissingVariable)
        );
    }

    #[test]
    fn json_escapes_only_variables_and_counts_expansion() {
        let value = "\"\\\n\r\t\u{0001}é${ctx:literal}";
        let mut session = MockPluginSession::new();
        session.expect_get_ctx_var().return_const(Some(value.to_string()));
        let tpl = compile(r#"{"value":"${ctx:x}"}"#);
        let escaped = serde_json::to_string(value).unwrap();
        let expected = format!("{{\"value\":{escaped}}}");
        for cap in [expected.len(), expected.len() - 1] {
            let result = render_template(
                &tpl,
                &session,
                RenderPolicy {
                    escape: EscapeMode::JsonString,
                    max_bytes: Some(cap),
                    ..RenderPolicy::EMPTY
                },
                SecretCapability::Denied,
            )
            .map(RenderedTemplate::into_string);
            if cap == expected.len() {
                assert_eq!(result.unwrap(), expected);
            } else {
                assert_eq!(result, Err(TemplateEvaluationError::TooLarge));
            }
        }
        for (value, bytes) in [("\"", 2), ("\\", 2), ("\u{0001}", 6), ("€", 3)] {
            let mut out = Output {
                value: String::new(),
                max_bytes: Some(bytes - 1),
            };
            assert_eq!(out.append_json(value), Err(TemplateEvaluationError::TooLarge));
            assert!(out.value.is_empty());
        }
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
            .expect_request_header_map()
            .return_const(http::HeaderMap::from_iter([(
                http::HeaderName::from_bytes(b"X-Tenant").unwrap(),
                http::HeaderValue::from_static("acme"),
            )]));
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
        session.expect_request_header_map().return_const(http::HeaderMap::new());
        let value = resolve_var(
            &TemplateVarType::Header,
            Some("X-Absent"),
            &session,
            SecretCapability::Denied,
        )
        .unwrap()
        .value
        .unwrap_or_default();
        assert_eq!(value, "");
    }

    #[test]
    fn test_resolve_present_variable() {
        let mut session = MockPluginSession::new();
        session
            .expect_get_query_param()
            .withf(|n| n == "id")
            .return_const(Some("42".to_string()));
        let value = resolve_var(&TemplateVarType::Query, Some("id"), &session, SecretCapability::Denied)
            .unwrap()
            .value
            .unwrap();
        assert_eq!(value, "42");
    }

    #[test]
    fn test_multi_variable_template() {
        let compiled = compile("${header:X-User}@${ctx:domain}");
        let mut session = MockPluginSession::new();
        session
            .expect_request_header_map()
            .return_const(http::HeaderMap::from_iter([(
                http::HeaderName::from_bytes(b"X-User").unwrap(),
                http::HeaderValue::from_static("alice"),
            )]));
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
        let v = resolve_var(
            &TemplateVarType::MatchedHttpRoute,
            Some("name"),
            &session,
            SecretCapability::Denied,
        )
        .unwrap()
        .value
        .unwrap();
        assert_eq!(v, "route1");
    }

    #[test]
    fn test_secret_ref_is_denied_in_session_facade() {
        let compiled = compile("x=${secretRef:ns/n:k}");
        let session = MockPluginSession::new();
        // Invalid artifacts cannot resolve Secrets or emit partial template output.
        assert_eq!(evaluate_template(&compiled, &session), "");
    }

    #[test]
    fn test_template_injection_prevention() {
        // A resolved value that itself contains `${…}` must be emitted verbatim,
        // never re-parsed as a nested directive.
        let compiled = compile("${header:X-Inject}");
        let mut session = MockPluginSession::new();
        session
            .expect_request_header_map()
            .return_const(http::HeaderMap::from_iter([(
                http::HeaderName::from_bytes(b"X-Inject").unwrap(),
                http::HeaderValue::from_static("${header:Authorization}"),
            )]));
        assert_eq!(evaluate_template(&compiled, &session), "${header:Authorization}");
    }

    #[test]
    fn bounded_template_rejects_missing_variable() {
        let compiled = compile("${header:X-Absent}");
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(http::HeaderMap::new());
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
            .expect_request_header_map()
            .return_const(http::HeaderMap::from_iter([(
                http::HeaderName::from_bytes(b"X-Value").unwrap(),
                http::HeaderValue::from_static("value"),
            )]));
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
