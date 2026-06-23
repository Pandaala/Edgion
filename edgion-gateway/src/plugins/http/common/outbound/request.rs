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
    CustomSource, ListFields, OutboundRequestSpec, PresencePolicy,
};
use edgion_resources::resources::edgion_plugins::plugin_configs::{CompiledTemplate, TemplateSegment, TemplateVarType};

use crate::plugins::http::common::http_client::{connection_listed_hop_by_hop, is_hop_by_hop_or_listed};
use crate::plugins::runtime::traits::session::RouteProto;
use crate::plugins::runtime::PluginSession;

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

        Ok(CompiledRequest {
            path,
            method,
            args,
            headers,
            cookies,
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

        Ok(req)
    }

    // ============================================================
    // collect_headers
    // ============================================================

    /// Collect outbound headers for the current session.
    ///
    /// When `self.headers.forward_all` is true, all incoming request headers
    /// are forwarded after stripping hop-by-hop headers (RFC 9110 §7.6.1),
    /// including those nominated by the `Connection:` header value.
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
                if !is_hop_by_hop_or_listed(&name, &dynamic) {
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

/// Evaluate a compiled template against the current session.
///
/// Missing variable values are silently substituted with an empty string —
/// the same convention used by the ctx_set plugin.
fn evaluate_template(
    tpl: &CompiledTemplate,
    session: &dyn PluginSession,
    resolved: Option<&ResolvedSecrets>,
) -> Result<String, String> {
    let mut result = String::with_capacity(tpl.hint_len());
    for seg in tpl.segments() {
        match seg {
            TemplateSegment::Literal(s) => result.push_str(s),
            TemplateSegment::Variable { var_type, var_name } => {
                if let Some(v) = resolve_var(var_type, var_name.as_deref(), session, resolved) {
                    result.push_str(&v);
                }
            }
        }
    }
    Ok(result)
}

/// Resolve a single template variable from the session.
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
        // Dynamically-listed hop-by-hop header must be stripped.
        assert!(
            result.get("x-custom-hop").is_none(),
            "x-custom-hop should be stripped (nominated by Connection header)"
        );
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
}
