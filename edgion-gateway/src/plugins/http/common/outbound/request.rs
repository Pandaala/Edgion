//! Pre-compiled outbound request builder for shared outbound-call engine.
//!
//! Templates and static values are compiled at resource-load time so that
//! the hot call path (`build_request`) only iterates over already-segmented
//! data — no per-call parsing.
//!
//! ## Lifecycle
//!
//! * **Load time** — `CompiledRequest::from_spec` parses templates.
//! * **Call time** — `build_request` evaluates compiled segments against the
//!   current `PluginSession` and the controller-carried `ResolvedSecrets` map.
//!
//! ## Secrets
//!
//! `${secretRef:ns/name:key}` segments are compiled at load time into
//! `CompiledCustomFieldSource::Template` entries. At call time, `build_request`
//! receives an `Option<&ResolvedSecrets>` map (keyed by `"ns/name:key"`) and
//! passes it through to `resolve_var`, which performs the lookup.

use std::collections::HashMap;

use edgion_resources::resources::common::external_http::ResolvedSecrets;
use edgion_resources::resources::common::outbound_request::{
    BodySpec, CustomSource, ListFields, OutboundRequestSpec, PresencePolicy,
};
use edgion_resources::resources::edgion_plugins::plugin_configs::{CompiledTemplate, TemplateSegment, TemplateVarType};

use crate::plugins::http::common::http_client::{connection_listed_hop_by_hop, is_hop_by_hop_or_listed};
use crate::plugins::runtime::traits::session::RouteProto;
use crate::plugins::runtime::PluginSession;

fn is_body_framing_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("transfer-encoding")
}

/// Whether `name` states an expectation the inbound hop already owns.
///
/// `Expect` is end-to-end rather than hop-by-hop, so hop-by-hop filtering does
/// not remove it. Relaying it is still wrong here: the outbound request carries
/// its own body (usually none), so the expectation describes an exchange that
/// will never happen on this connection, and a strict server answers
/// `417 Expectation Failed` — which ForwardAuth reads as a denied request.
/// The inbound expectation is settled separately by the body-capture path,
/// which writes the `100 Continue` response itself and then strips the header.
fn is_hop_owned_expectation_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("expect")
}

/// Rebuild framing for the body owned by this outbound HTTP hop.
///
/// Incoming framing is never reusable here: the configured method and body can
/// differ from the downstream request. All current outbound body variants are
/// replayable byte bodies, so their exact length is known before dispatch.
pub(super) fn sync_outbound_body_framing(req: &mut reqwest::Request) -> Result<(), String> {
    req.headers_mut().remove(reqwest::header::CONTENT_LENGTH);
    req.headers_mut().remove(reqwest::header::TRANSFER_ENCODING);

    let Some(body) = req.body().and_then(reqwest::Body::as_bytes) else {
        return Ok(());
    };
    let len = reqwest::header::HeaderValue::from_str(&body.len().to_string())
        .map_err(|err| format!("invalid outbound Content-Length: {err}"))?;
    req.headers_mut().insert(reqwest::header::CONTENT_LENGTH, len);
    Ok(())
}

// ============================================================
// Public compiled types
// ============================================================

/// Pre-compiled outbound request specification.
///
/// Built once at resource-load time from an [`OutboundRequestSpec`].
/// The `build_request` method evaluates the compiled templates at call time.
/// The `collect_headers` method evaluates the header allow-list at call time,
/// including `forward_all` semantics.
#[derive(Debug, Clone)]
pub struct CompiledRequest {
    /// Compiled path specification. `None` = use frozen incoming path.
    pub path: Option<CompiledPathSpec>,
    /// Compiled method template. `None` = use frozen incoming method.
    pub method: Option<CompiledTemplate>,
    /// Compiled query-argument allow-list.
    pub args: CompiledListFields,
    /// Compiled header allow-list.
    pub headers: CompiledListFields,
    /// Compiled cookie allow-list.
    pub cookies: CompiledListFields,
    /// Compiled request body. `None` = no body (existing behavior).
    pub body: Option<CompiledBody>,
}

/// Compiled path specification (from [`PathSpec`]).
#[derive(Debug, Clone)]
pub struct CompiledPathSpec {
    /// Pre-compiled path template.
    pub template: CompiledTemplate,
    /// Whether `path_override` supplied to `build_request` is honoured.
    pub allow_override: bool,
}

/// Compiled allow-listed fields (forward_all flag + origin + custom).
#[derive(Debug, Clone)]
pub struct CompiledListFields {
    /// When true, all incoming fields are forwarded (minus hop-by-hop for headers).
    pub forward_all: bool,
    /// Fields forwarded from the incoming request (frozen at entry time).
    pub origin: Vec<CompiledOriginField>,
    /// Fields explicitly constructed by the operator.
    pub custom: Vec<CompiledCustomField>,
}

/// Compiled origin field: name + presence policy.
#[derive(Debug, Clone)]
pub struct CompiledOriginField {
    /// Field name (header name, query param name, or cookie name).
    pub name: String,
    /// What to do when the incoming request lacks this field.
    pub presence: PresencePolicy,
}

/// Compiled custom field: name + resolved source.
#[derive(Debug, Clone)]
pub struct CompiledCustomField {
    /// Field name in the outbound request.
    pub name: String,
    /// Source of the field value (compiled at load time).
    pub source: CompiledCustomFieldSource,
}

/// Compiled source for a custom field.
#[derive(Debug, Clone)]
pub enum CompiledCustomFieldSource {
    /// Verbatim string literal emitted as-is on every call.
    Literal(String),
    /// Template compiled at load time; `${secretRef:...}` segments are resolved
    /// at call time from the carried `resolved_secrets` map.
    Template(CompiledTemplate),
}

/// Compiled request body (from [`BodySpec`]).
#[derive(Debug, Clone)]
pub enum CompiledBody {
    /// JSON template; variable values are JSON-string-escaped at eval time.
    Json(CompiledTemplate),
    /// Form fields, percent-encoded at eval time.
    Form(Vec<CompiledCustomField>),
    /// Verbatim template.
    Raw(CompiledTemplate),
}

// ============================================================
// from_spec
// ============================================================

impl CompiledRequest {
    /// Compile an [`OutboundRequestSpec`] into a `CompiledRequest`.
    ///
    /// Templates are parsed once at load time. `${secretRef:...}` segments are
    /// stored as `Template` entries and resolved at call time from the
    /// `resolved_secrets` map passed to `build_request`.
    ///
    /// Returns `Err` if any template string fails to parse.
    pub fn from_spec(spec: &OutboundRequestSpec) -> Result<Self, String> {
        let path = spec
            .path
            .as_ref()
            .map(|ps| {
                Ok::<_, String>(CompiledPathSpec {
                    template: CompiledTemplate::compile(&ps.template)?,
                    allow_override: ps.allow_override,
                })
            })
            .transpose()?;

        let method = spec
            .method
            .as_ref()
            .map(|ms| CompiledTemplate::compile(&ms.template))
            .transpose()?;

        let args = compile_list_fields(&spec.args)?;
        let headers = compile_list_fields(&spec.headers)?;
        let cookies = compile_list_fields(&spec.cookies)?;

        let body = spec
            .body
            .as_ref()
            .map(|b| {
                Ok::<_, String>(match b {
                    BodySpec::Json { template } => CompiledBody::Json(CompiledTemplate::compile(template)?),
                    BodySpec::Raw { template } => CompiledBody::Raw(CompiledTemplate::compile(template)?),
                    BodySpec::Form { fields } => {
                        let compiled = fields
                            .iter()
                            .map(|f| {
                                let source = match &f.source {
                                    CustomSource::Value(v) => CompiledCustomFieldSource::Literal(v.clone()),
                                    CustomSource::Template(t) => {
                                        CompiledCustomFieldSource::Template(CompiledTemplate::compile(t)?)
                                    }
                                };
                                Ok::<_, String>(CompiledCustomField {
                                    name: f.name.clone(),
                                    source,
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        CompiledBody::Form(compiled)
                    }
                })
            })
            .transpose()?;

        Ok(CompiledRequest {
            path,
            method,
            args,
            headers,
            cookies,
            body,
        })
    }

    // ============================================================
    // build_request
    // ============================================================

    /// Build a `reqwest::Request` for the given session and call parameters.
    ///
    /// * `session`       — provides incoming-request fields for origin lookups
    ///                     and template variable resolution.
    /// * `path_override` — caller-supplied path suffix; honoured only when
    ///                     the compiled path spec has `allow_override = true`.
    ///                     Ignored (not an error) when override is disallowed.
    /// * `base_url`      — service base URL (`scheme://host[:port]`).
    ///                     A trailing `/` is stripped before appending the path.
    ///
    /// Returns `Err` if:
    /// - a required origin field is absent from the incoming request,
    /// - the resulting URL or HTTP method string is invalid.
    pub fn build_request(
        &self,
        session: &dyn PluginSession,
        path_override: Option<&str>,
        base_url: &str,
        resolved: Option<&ResolvedSecrets>,
    ) -> Result<reqwest::Request, String> {
        // ── 1. Determine path ──────────────────────────────────────────────
        let final_path = match &self.path {
            Some(path_spec) => {
                if path_spec.allow_override {
                    if let Some(ov) = path_override {
                        ov.to_string()
                    } else {
                        evaluate_template(&path_spec.template, session, resolved)?
                    }
                } else {
                    evaluate_template(&path_spec.template, session, resolved)?
                }
            }
            // No path spec: use the frozen entry-time path.
            None => session.get_request_path().to_string(),
        };

        // ── 2. Determine method ───────────────────────────────────────────
        let final_method = match &self.method {
            Some(method_tpl) => evaluate_template(method_tpl, session, resolved)?,
            None => session.get_method().to_string(),
        };

        // ── 3. Collect query args ─────────────────────────────────────────
        let mut query_pairs: Vec<(String, String)> = Vec::new();
        collect_list_fields(&self.args, session, FieldKind::Query, &mut query_pairs, resolved)?;

        // ── 4. Build URL ──────────────────────────────────────────────────
        let url_base = base_url.trim_end_matches('/');
        let path_part = if final_path.starts_with('/') {
            final_path.clone()
        } else {
            format!("/{}", final_path)
        };
        let url_str = format!("{}{}", url_base, path_part);
        let mut url =
            reqwest::Url::parse(&url_str).map_err(|e| format!("invalid outbound URL {:?}: {}", url_str, e))?;

        if !query_pairs.is_empty() {
            let mut qp = url.query_pairs_mut();
            for (k, v) in &query_pairs {
                qp.append_pair(k, v);
            }
        }

        // ── 5. Parse HTTP method ──────────────────────────────────────────
        let method = reqwest::Method::from_bytes(final_method.as_bytes())
            .map_err(|e| format!("invalid HTTP method {:?}: {}", final_method, e))?;

        // ── 6. Create request ─────────────────────────────────────────────
        let mut req = reqwest::Request::new(method, url);

        // ── 7. Add headers ────────────────────────────────────────────────
        // Use collect_headers so that forward_all semantics are honoured:
        // when forward_all=true all non-hop-by-hop headers from the incoming
        // request are forwarded, and explicit origin/custom entries are applied
        // on top (possibly overriding). When forward_all=false only the
        // explicit origin/custom entries are sent.
        let header_map = self.collect_headers(session, resolved)?;
        for (k, v) in header_map {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(&v),
            ) {
                req.headers_mut().insert(name, val);
            }
            // Silently skip headers with invalid names or values (data-plane rule).
        }

        // ── 8. Add cookies as a single Cookie header ──────────────────────
        let mut cookie_pairs: Vec<(String, String)> = Vec::new();
        collect_list_fields(&self.cookies, session, FieldKind::Cookie, &mut cookie_pairs, resolved)?;
        if !cookie_pairs.is_empty() {
            let cookie_str = cookie_pairs
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join("; ");
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&cookie_str) {
                req.headers_mut().insert(reqwest::header::COOKIE, val);
            }
            // Silently skip Cookie header with invalid value (data-plane rule).
        }

        // ── 9. Body (json/form/raw) ───────────────────────────────────────
        // Runs after all header assembly so the body-derived Content-Type
        // overrides a forward_all-forwarded incoming value. An explicit custom
        // Content-Type cannot coexist with json/form (rejected at config time).
        if let Some(body) = &self.body {
            match body {
                CompiledBody::Json(tpl) => {
                    let rendered = evaluate_template_with(tpl, session, resolved, VarEscape::Json)?;
                    *req.body_mut() = Some(rendered.into());
                    req.headers_mut().insert(
                        reqwest::header::CONTENT_TYPE,
                        reqwest::header::HeaderValue::from_static("application/json"),
                    );
                }
                CompiledBody::Form(fields) => {
                    let mut ser = url::form_urlencoded::Serializer::new(String::new());
                    for f in fields {
                        let value = match &f.source {
                            CompiledCustomFieldSource::Literal(v) => v.clone(),
                            CompiledCustomFieldSource::Template(tpl) => evaluate_template(tpl, session, resolved)?,
                        };
                        ser.append_pair(&f.name, &value);
                    }
                    *req.body_mut() = Some(ser.finish().into());
                    req.headers_mut().insert(
                        reqwest::header::CONTENT_TYPE,
                        reqwest::header::HeaderValue::from_static("application/x-www-form-urlencoded"),
                    );
                }
                CompiledBody::Raw(tpl) => {
                    let rendered = evaluate_template(tpl, session, resolved)?;
                    *req.body_mut() = Some(rendered.into());
                }
            }
        }

        sync_outbound_body_framing(&mut req)?;
        Ok(req)
    }

    // ============================================================
    // collect_headers
    // ============================================================

    /// Collect outbound headers for the current session.
    ///
    /// When `self.headers.forward_all` is true, all incoming request headers
    /// are forwarded after stripping hop-by-hop headers (RFC 9110 §7.6.1),
    /// proxy-owned body framing, `Expect`, and names nominated by `Connection`.
    /// Body framing and `Expect` are end-to-end headers withheld deliberately:
    /// they describe the inbound message, not the one being built here.
    ///
    /// Explicit `origin` and `custom` fields are then applied on top, allowing
    /// operators to override or inject specific headers.
    ///
    /// Returns `Ok` with a `HashMap` (lowercase header names as keys) on success,
    /// or `Err` if a `Required` origin header is absent from the incoming request.
    pub fn collect_headers(
        &self,
        session: &dyn PluginSession,
        resolved: Option<&ResolvedSecrets>,
    ) -> Result<HashMap<String, String>, String> {
        let mut result = HashMap::new();

        // forward_all: add all session headers minus hop-by-hop (RFC 9110 §7.6.1).
        if self.headers.forward_all {
            let conn_val = session.header_value("connection").unwrap_or_default();
            let dynamic = connection_listed_hop_by_hop(&conn_val);
            for (name, value) in session.request_headers() {
                if !is_hop_by_hop_or_listed(&name, &dynamic)
                    && !is_body_framing_header(&name)
                    && !is_hop_owned_expectation_header(&name)
                {
                    result.insert(name.to_ascii_lowercase(), value);
                }
            }
        }

        // Apply explicit origin + custom fields (may override forward_all entries).
        // Propagate Required-field errors rather than swallowing them.
        // Pass resolved so that ${secretRef:...} templates in custom fields resolve.
        let mut pairs: Vec<(String, String)> = Vec::new();
        collect_list_fields(&self.headers, session, FieldKind::Header, &mut pairs, resolved)?;
        for (k, v) in pairs {
            result.insert(k.to_ascii_lowercase(), v);
        }

        Ok(result)
    }
}

// ============================================================
// Internal helpers
// ============================================================

/// Which bucket of the incoming request to look up origin fields from.
enum FieldKind {
    Header,
    Query,
    Cookie,
}

/// Compile a [`ListFields`] specification into its pre-parsed form.
fn compile_list_fields(fields: &ListFields) -> Result<CompiledListFields, String> {
    let origin = fields
        .origin
        .iter()
        .map(|f| {
            Ok::<_, String>(CompiledOriginField {
                name: f.name.clone(),
                presence: f.presence,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let custom = fields
        .custom
        .iter()
        .map(|f| {
            let source = match &f.source {
                CustomSource::Value(v) => CompiledCustomFieldSource::Literal(v.clone()),
                CustomSource::Template(t) => CompiledCustomFieldSource::Template(CompiledTemplate::compile(t)?),
            };
            Ok::<_, String>(CompiledCustomField {
                name: f.name.clone(),
                source,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CompiledListFields {
        forward_all: fields.forward_all,
        origin,
        custom,
    })
}

/// How to escape a resolved variable value before splicing into the output.
#[derive(Clone, Copy)]
enum VarEscape {
    /// No escaping — path/method/args/headers/cookies (their sinks escape).
    None,
    /// JSON string escaping (RFC 8259 section 7) for json body templates.
    Json,
}

/// Append `value` to `out`, escaping JSON string special characters.
fn push_json_escaped(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// Evaluate a compiled template against the current session.
///
/// Missing variable values are silently substituted with an empty string —
/// the same convention used by the ctx_set plugin.
pub(crate) fn evaluate_template(
    tpl: &CompiledTemplate,
    session: &dyn PluginSession,
    resolved: Option<&ResolvedSecrets>,
) -> Result<String, String> {
    evaluate_template_with(tpl, session, resolved, VarEscape::None)
}

/// Evaluate a compiled template, escaping each variable value per `escape`.
/// Literal segments are never escaped. Missing variables render as empty
/// string (ctx_set convention).
fn evaluate_template_with(
    tpl: &CompiledTemplate,
    session: &dyn PluginSession,
    resolved: Option<&ResolvedSecrets>,
    escape: VarEscape,
) -> Result<String, String> {
    let mut result = String::with_capacity(tpl.hint_len());
    for seg in tpl.segments() {
        match seg {
            TemplateSegment::Literal(s) => result.push_str(s),
            TemplateSegment::Variable { var_type, var_name } => {
                if let Some(v) = resolve_var(var_type, var_name.as_deref(), session, resolved) {
                    match escape {
                        VarEscape::None => result.push_str(&v),
                        VarEscape::Json => push_json_escaped(&mut result, &v),
                    }
                }
            }
        }
    }
    Ok(result)
}

/// Resolve a single template variable from the session.
///
/// Private: the only caller is `evaluate_template_with` in this module —
/// `evaluate_template` itself is `pub(crate)` for cross-module callers (e.g.
/// `outbound::call::resolve_timeout`), but `resolve_var` has no cross-module
/// caller and does not need the wider visibility.
fn resolve_var(
    var_type: &TemplateVarType,
    var_name: Option<&str>,
    session: &dyn PluginSession,
    resolved: Option<&ResolvedSecrets>,
) -> Option<String> {
    match var_type {
        TemplateVarType::Header => session.header_value(var_name?),
        TemplateVarType::Query => session.get_query_param(var_name?),
        TemplateVarType::Cookie => session.get_cookie(var_name?),
        TemplateVarType::Ctx => session.get_ctx_var(var_name?),
        TemplateVarType::Path => Some(session.get_path().to_string()),
        TemplateVarType::Uri => Some(session.get_uri()),
        TemplateVarType::RequestPath => Some(session.get_request_path().to_string()),
        TemplateVarType::RequestUri => Some(session.get_request_uri()),
        TemplateVarType::Method => Some(session.get_method().to_string()),
        TemplateVarType::ClientIp => {
            let addr = session.remote_addr();
            if addr.is_empty() {
                None
            } else {
                Some(addr.to_string())
            }
        }
        TemplateVarType::MatchedHttpRoute => session.matched_route_meta(RouteProto::Http, var_name?),
        TemplateVarType::MatchedGrpcRoute => session.matched_route_meta(RouteProto::Grpc, var_name?),
        // Resolved at call time from the controller-carried map (keyed by the
        // raw var_name "ns/name:key"). Absent → None.
        TemplateVarType::SecretRef => resolved.and_then(|r| r.0.get(var_name?).cloned()),
    }
}

/// Collect resolved field key-value pairs from `fields` into `output`.
///
/// Origin fields are read from the incoming request via the session.
/// Custom fields are evaluated from their compiled source.
///
/// Note: this function does NOT process `forward_all`; that is handled by
/// [`CompiledRequest::collect_headers`] before calling this helper.
fn collect_list_fields(
    fields: &CompiledListFields,
    session: &dyn PluginSession,
    kind: FieldKind,
    output: &mut Vec<(String, String)>,
    resolved: Option<&ResolvedSecrets>,
) -> Result<(), String> {
    // Origin fields: forwarded from the incoming request.
    for origin in &fields.origin {
        let value = get_session_field(session, &kind, &origin.name);
        match value {
            Some(v) => output.push((origin.name.clone(), v)),
            None => match origin.presence {
                PresencePolicy::SkipIfMissing | PresencePolicy::WarnIfMissing => {
                    // Silently omit (data-plane rule: no tracing in data plane).
                }
                PresencePolicy::Required => {
                    return Err(format!(
                        "required outbound field {:?} is missing from incoming request",
                        origin.name
                    ));
                }
            },
        }
    }

    // Custom fields: injected by the operator.
    for custom in &fields.custom {
        let value = match &custom.source {
            CompiledCustomFieldSource::Literal(v) => v.clone(),
            CompiledCustomFieldSource::Template(tpl) => evaluate_template(tpl, session, resolved)?,
        };
        output.push((custom.name.clone(), value));
    }

    Ok(())
}

/// Extract a single named field from the session based on field kind.
fn get_session_field(session: &dyn PluginSession, kind: &FieldKind, name: &str) -> Option<String> {
    match kind {
        FieldKind::Header => session.header_value(name),
        FieldKind::Query => session.get_query_param(name),
        FieldKind::Cookie => session.get_cookie(name),
    }
}

// ============================================================
// Unit tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::common::outbound_request::ListFields;

    /// TDD test (written before implementation): `forward_all=true` forwards
    /// non-hop-by-hop headers and strips both static and dynamically-listed
    /// hop-by-hop headers (RFC 9110 §7.6.1).
    ///
    /// RED: fails because `collect_headers` returns an empty map.
    /// GREEN: passes after the full implementation is wired in.
    #[test]
    fn forward_all_includes_normal_strips_hop_by_hop() {
        use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

        let spec = OutboundRequestSpec {
            headers: ListFields {
                forward_all: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        // Connection header nominates "X-Custom-Hop" as an additional hop-by-hop name.
        session
            .expect_header_value()
            .withf(|name| name == "connection")
            .return_const(Some("X-Custom-Hop".to_string()));
        // All request headers returned by the session, including hop-by-hop and normal ones.
        session.expect_request_headers().return_const(vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("content-length".to_string(), "9".to_string()),
            ("transfer-encoding".to_string(), "chunked".to_string()),
            ("connection".to_string(), "X-Custom-Hop".to_string()),
            ("keep-alive".to_string(), "timeout=5".to_string()),
            ("x-custom-hop".to_string(), "remove-this".to_string()),
            ("authorization".to_string(), "Bearer token123".to_string()),
        ]);

        let result = compiled.collect_headers(&session, None).unwrap();

        // Non-hop-by-hop headers must be present.
        assert_eq!(
            result.get("content-type").map(String::as_str),
            Some("application/json"),
            "content-type should be forwarded"
        );
        assert_eq!(
            result.get("authorization").map(String::as_str),
            Some("Bearer token123"),
            "authorization should be forwarded"
        );
        // Static hop-by-hop headers must be stripped.
        assert!(
            result.get("connection").is_none(),
            "connection should be stripped (static hop-by-hop)"
        );
        assert!(
            result.get("keep-alive").is_none(),
            "keep-alive should be stripped (static hop-by-hop)"
        );
        assert!(
            result.get("content-length").is_none(),
            "content-length belongs to the outbound body, not the incoming request"
        );
        assert!(
            result.get("transfer-encoding").is_none(),
            "transfer-encoding belongs to the outbound transport"
        );
        // Dynamically-listed hop-by-hop header must be stripped.
        assert!(
            result.get("x-custom-hop").is_none(),
            "x-custom-hop should be stripped (nominated by Connection header)"
        );
    }

    #[test]
    fn bodyless_outbound_request_does_not_inherit_incoming_content_length() {
        use edgion_resources::resources::common::outbound_request::{MethodSpec, OutboundRequestSpec, PathSpec};

        let spec = OutboundRequestSpec {
            path: Some(PathSpec {
                template: "/verify".to_string(),
                allow_override: false,
            }),
            method: Some(MethodSpec {
                template: "GET".to_string(),
            }),
            headers: ListFields {
                forward_all: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "connection")
            .return_const(None::<String>);
        session.expect_request_headers().return_const(vec![
            ("authorization".to_string(), "Bearer valid-token".to_string()),
            ("content-length".to_string(), "9".to_string()),
        ]);

        let req = compiled
            .build_request(&session, None, "http://auth.test", None)
            .unwrap();

        assert!(req.body().is_none());
        assert!(req.headers().get(reqwest::header::CONTENT_LENGTH).is_none());
        assert_eq!(
            req.headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer valid-token")
        );
    }

    #[test]
    fn body_framing_sync_replaces_stale_length_and_transfer_encoding() {
        let mut req = reqwest::Request::new(
            reqwest::Method::POST,
            reqwest::Url::parse("http://upstream.test/").unwrap(),
        );
        req.headers_mut().insert(
            reqwest::header::CONTENT_LENGTH,
            reqwest::header::HeaderValue::from_static("99"),
        );
        req.headers_mut().insert(
            reqwest::header::TRANSFER_ENCODING,
            reqwest::header::HeaderValue::from_static("chunked"),
        );
        *req.body_mut() = Some(bytes::Bytes::from_static(b"abc").into());

        sync_outbound_body_framing(&mut req).unwrap();

        assert_eq!(
            req.headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok()),
            Some("3")
        );
        assert!(req.headers().get(reqwest::header::TRANSFER_ENCODING).is_none());
    }

    /// C2 regression: `forward_all=true` with a `Required` origin field whose
    /// header is absent from the session must return `Err`, not silently omit it.
    ///
    /// Before the fix, `collect_headers` used `let _ = collect_list_fields(...)`,
    /// which swallowed the error and downgraded `Required` to `SkipIfMissing`.
    #[test]
    fn forward_all_with_required_origin_missing_returns_error() {
        use edgion_resources::resources::common::outbound_request::{OriginField, OutboundRequestSpec, PresencePolicy};

        let spec = OutboundRequestSpec {
            headers: ListFields {
                forward_all: true,
                origin: vec![OriginField {
                    name: "X-Required".to_string(),
                    presence: PresencePolicy::Required,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        // No "connection" header — connection_listed_hop_by_hop returns empty set.
        session
            .expect_header_value()
            .withf(|name| name == "connection")
            .return_const(None::<String>);
        // X-Required is NOT present in session headers.
        session
            .expect_header_value()
            .withf(|name| name == "X-Required")
            .return_const(None::<String>);
        // request_headers does not include X-Required.
        session
            .expect_request_headers()
            .return_const(vec![("content-type".to_string(), "text/plain".to_string())]);

        let result = compiled.collect_headers(&session, None);
        assert!(result.is_err(), "Expected Err for missing Required header, got Ok");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Required") || msg.contains("required"),
            "Error message should mention Required; got: {msg}"
        );
    }

    /// `Expect` is end-to-end, so hop-by-hop filtering keeps it. `forward_all`
    /// must still withhold it: the outbound auth call sends no body of its own,
    /// and a server enforcing the expectation answers `417 Expectation Failed`,
    /// which ForwardAuth would read as a denied request.
    #[test]
    fn forward_all_strips_expect_continue() {
        use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

        let spec = OutboundRequestSpec {
            headers: ListFields {
                forward_all: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "connection")
            .return_const(None::<String>);
        session.expect_request_headers().return_const(vec![
            ("expect".to_string(), "100-continue".to_string()),
            ("authorization".to_string(), "Bearer token123".to_string()),
        ]);

        let result = compiled.collect_headers(&session, None).unwrap();

        assert!(
            !result.contains_key("expect"),
            "expect states an expectation owned by the inbound hop"
        );
        assert_eq!(
            result.get("authorization").map(String::as_str),
            Some("Bearer token123"),
            "unrelated headers must still be forwarded"
        );
    }

    /// The automatic skip covers `forward_all` only. Naming `Expect` in `origin`
    /// is a deliberate operator act and keeps working, matching how the other
    /// non-hop-by-hop skips treat an explicit listing.
    #[test]
    fn explicitly_listed_origin_expect_is_still_forwarded() {
        use edgion_resources::resources::common::outbound_request::{OriginField, OutboundRequestSpec, PresencePolicy};

        let spec = OutboundRequestSpec {
            headers: ListFields {
                forward_all: true,
                origin: vec![OriginField {
                    name: "Expect".to_string(),
                    presence: PresencePolicy::SkipIfMissing,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "connection")
            .return_const(None::<String>);
        // Distinct values per lookup path so the assertion identifies which branch
        // produced the entry: the origin lookup, not the forward_all sweep.
        session
            .expect_header_value()
            .withf(|name| name == "Expect")
            .return_const(Some("100-continue-via-origin".to_string()));
        session
            .expect_request_headers()
            .return_const(vec![("expect".to_string(), "100-continue".to_string())]);

        let result = compiled.collect_headers(&session, None).unwrap();

        assert_eq!(
            result.get("expect").map(String::as_str),
            Some("100-continue-via-origin"),
            "an explicitly listed origin field overrides the forward_all skip"
        );
    }

    use edgion_resources::resources::common::outbound_request::{
        BodySpec, CustomField, CustomSource, OutboundRequestSpec,
    };

    /// Build a spec with fixed path "/wh" + method "POST" so the session mock
    /// only needs expectations for the variables the body actually reads.
    fn body_spec(body: BodySpec) -> OutboundRequestSpec {
        use edgion_resources::resources::common::outbound_request::{MethodSpec, PathSpec};
        OutboundRequestSpec {
            path: Some(PathSpec {
                template: "/wh".to_string(),
                allow_override: false,
            }),
            method: Some(MethodSpec {
                template: "POST".to_string(),
            }),
            body: Some(body),
            ..Default::default()
        }
    }

    fn req_body_string(req: &reqwest::Request) -> String {
        String::from_utf8(req.body().unwrap().as_bytes().unwrap().to_vec()).unwrap()
    }

    #[test]
    fn json_body_escapes_variable_values_and_sets_content_type() {
        let spec = body_spec(BodySpec::Json {
            template: r#"{"user": "${header:x-user}", "source": "edgion"}"#.to_string(),
        });
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        // Value contains a quote, a backslash, and a newline — all must be escaped.
        session
            .expect_header_value()
            .withf(|n| n == "x-user")
            .return_const(Some("ali\"ce\\bob\nline2".to_string()));

        let req = compiled
            .build_request(&session, None, "http://upstream.test", None)
            .unwrap();

        let body = req_body_string(&req);
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("rendered body must be valid JSON");
        assert_eq!(parsed["user"], "ali\"ce\\bob\nline2");
        assert_eq!(parsed["source"], "edgion");
        assert_eq!(
            req.headers().get("content-type").map(|v| v.to_str().unwrap()),
            Some("application/json")
        );
        assert_eq!(
            req.headers().get("content-length").map(|v| v.to_str().unwrap()),
            Some(body.as_bytes().len().to_string().as_str())
        );
    }

    #[test]
    fn form_body_percent_encodes_values() {
        let spec = body_spec(BodySpec::Form {
            fields: vec![
                CustomField {
                    name: "userId".to_string(),
                    source: CustomSource::Template("${ctx:uid}".to_string()),
                },
                CustomField {
                    name: "source".to_string(),
                    source: CustomSource::Value("edgion".to_string()),
                },
            ],
        });
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        session
            .expect_get_ctx_var()
            .withf(|n| n == "uid")
            .return_const(Some("a&b=c d".to_string()));

        let req = compiled
            .build_request(&session, None, "http://upstream.test", None)
            .unwrap();

        assert_eq!(req_body_string(&req), "userId=a%26b%3Dc+d&source=edgion");
        assert_eq!(
            req.headers().get("content-type").map(|v| v.to_str().unwrap()),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(
            req.headers().get("content-length").map(|v| v.to_str().unwrap()),
            Some(req_body_string(&req).len().to_string().as_str())
        );
    }

    #[test]
    fn raw_body_is_verbatim_without_content_type() {
        let spec = body_spec(BodySpec::Raw {
            template: "plain ${ctx:x} text".to_string(),
        });
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        session
            .expect_get_ctx_var()
            .withf(|n| n == "x")
            .return_const(Some("a\"b".to_string()));

        let req = compiled
            .build_request(&session, None, "http://upstream.test", None)
            .unwrap();

        // Raw: no escaping, no auto Content-Type.
        assert_eq!(req_body_string(&req), "plain a\"b text");
        assert!(req.headers().get("content-type").is_none());
        assert_eq!(
            req.headers().get("content-length").map(|v| v.to_str().unwrap()),
            Some("14")
        );
    }

    #[test]
    fn json_body_content_type_overrides_forwarded_incoming() {
        use edgion_resources::resources::common::outbound_request::ListFields;
        let mut spec = body_spec(BodySpec::Json {
            template: r#"{"a": "1"}"#.to_string(),
        });
        spec.headers = ListFields {
            forward_all: true,
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|n| n == "connection")
            .return_const(None::<String>);
        session
            .expect_request_headers()
            .return_const(vec![("content-type".to_string(), "text/plain".to_string())]);

        let req = compiled
            .build_request(&session, None, "http://upstream.test", None)
            .unwrap();

        // The body-derived Content-Type must win over the forwarded incoming one.
        assert_eq!(
            req.headers().get("content-type").map(|v| v.to_str().unwrap()),
            Some("application/json")
        );
        assert_eq!(
            req.headers().get("content-length").map(|v| v.to_str().unwrap()),
            Some("10")
        );
    }
}
