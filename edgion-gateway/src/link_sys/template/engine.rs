//! Canonical request-template runtime: one segment walk, explicit consumer policies.
//!
//! Compiled segments are never reparsed. The output budget bounds rendered UTF-8
//! bytes, not owned source values returned by session accessors or allocator rounding.

use crate::plugins::runtime::{traits::session::RouteProto, PluginSession};
use edgion_resources::resources::common::{
    external_http::ResolvedSecrets,
    request_template::{CompiledTemplate, TemplateSegment, TemplateVarType},
};

/// Trusted values available without borrowing downstream transport state.
pub trait TrustedTemplateVariables: Send + Sync {
    fn trusted_ctx_var(&self, key: &str) -> Option<String>;
    fn trusted_remote_ip(&self) -> &str;
    fn trusted_matched_route_meta(&self, proto: RouteProto, selector: &str) -> Option<String>;
}

impl<T: PluginSession + ?Sized> TrustedTemplateVariables for T {
    fn trusted_ctx_var(&self, key: &str) -> Option<String> {
        self.get_ctx_var(key)
    }

    fn trusted_remote_ip(&self) -> &str {
        self.remote_ip()
    }

    fn trusted_matched_route_meta(&self, proto: RouteProto, selector: &str) -> Option<String> {
        self.matched_route_meta(proto, selector)
    }
}

/// Explicit read capabilities for the canonical template renderer.
#[derive(Clone, Copy)]
pub enum TemplateContext<'a> {
    Request(&'a dyn PluginSession),
    Trusted(&'a dyn TrustedTemplateVariables),
}

impl<'a> TemplateContext<'a> {
    pub(crate) fn request_session(self) -> Result<&'a dyn PluginSession, TemplateEvaluationError> {
        match self {
            Self::Request(session) => Ok(session),
            Self::Trusted(_) => Err(TemplateEvaluationError::UnavailableVariable),
        }
    }

    fn ctx_var(self, key: &str) -> Option<String> {
        match self {
            Self::Request(session) => session.get_ctx_var(key),
            Self::Trusted(values) => values.trusted_ctx_var(key),
        }
    }

    fn remote_ip(self) -> &'a str {
        match self {
            Self::Request(session) => session.remote_ip(),
            Self::Trusted(values) => values.trusted_remote_ip(),
        }
    }

    fn matched_route_meta(self, proto: RouteProto, selector: &str) -> Option<String> {
        match self {
            Self::Request(session) => session.matched_route_meta(proto, selector),
            Self::Trusted(values) => values.trusted_matched_route_meta(proto, selector),
        }
    }
}

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
    UnavailableVariable,
}

impl std::fmt::Display for TemplateEvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingVariable => "template variable missing",
            Self::TooLarge => "template output too large",
            Self::SecretNotAllowed => "template Secret capability denied",
            Self::UnavailableVariable => "template request capability unavailable",
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
    render_template_in(compiled, TemplateContext::Request(session), policy, secrets)
}

/// Render using only the explicitly supplied request or trusted capabilities.
pub fn render_template_in(
    compiled: &CompiledTemplate,
    context: TemplateContext<'_>,
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
                let resolved = resolve_var(var_type, var_name.as_deref(), context, secrets)?;
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
    context: TemplateContext<'_>,
    secrets: SecretCapability<'_>,
) -> Result<ResolvedVariable, TemplateEvaluationError> {
    // Cookie accessors do not expose per-cookie metadata. Preserve the same
    // conservative protection as reconstructed outbound Cookie headers.
    let mut sensitive = matches!(var_type, TemplateVarType::SecretRef | TemplateVarType::Cookie);
    let value = match var_type {
        TemplateVarType::Header => {
            let headers = context.request_session()?.request_header_map();
            name.and_then(|name| headers.get(name)).and_then(|value| {
                sensitive = value.is_sensitive();
                value.to_str().ok().map(str::to_string)
            })
        }
        TemplateVarType::Query => {
            let session = context.request_session()?;
            name.and_then(|name| session.get_query_param(name))
        }
        TemplateVarType::Cookie => {
            let session = context.request_session()?;
            name.and_then(|name| session.get_cookie(name))
        }
        TemplateVarType::Ctx => name.and_then(|n| context.ctx_var(n)),
        TemplateVarType::Path => Some(context.request_session()?.get_path().to_string()),
        TemplateVarType::Uri => Some(context.request_session()?.get_uri()),
        TemplateVarType::RequestPath => Some(context.request_session()?.get_request_path().to_string()),
        TemplateVarType::RequestUri => Some(context.request_session()?.get_request_uri()),
        TemplateVarType::Method => Some(context.request_session()?.get_method().to_string()),
        TemplateVarType::RemoteIp => {
            let ip = context.remote_ip();
            (!ip.is_empty()).then(|| ip.to_string())
        }
        TemplateVarType::MatchedHttpRoute => name.and_then(|n| context.matched_route_meta(RouteProto::Http, n)),
        TemplateVarType::MatchedGrpcRoute => name.and_then(|n| context.matched_route_meta(RouteProto::Grpc, n)),
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
#[path = "../../../tests/unit/link_sys/template/engine_test.rs"]
mod tests;
