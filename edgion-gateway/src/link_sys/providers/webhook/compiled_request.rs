//! Pre-compiled outbound webhook request builder.
//!
//! Templates and static values are compiled at resource-load time so that
//! the hot call path (`build_request`) only iterates over already-segmented
//! data — no per-call parsing.
//!
//! ## Lifecycle
//!
//! * **Load time** — `CompiledOutboundRequest::from_spec` parses templates
//!   and stores the compiled result.
//! * **Call time** — `build_request` evaluates the compiled segments against
//!   the current `PluginSession` and the controller-carried `ResolvedSecrets`
//!   map, and returns a `reqwest::Request`.
//!
//! ## Secrets
//!
//! `${secretRef:ns/name:key}` segments are compiled at load time into
//! `CompiledCustomFieldSource::Template` entries.  At call time, `build_request`
//! receives an `Option<&ResolvedSecrets>` map (keyed by `"ns/name:key"`) and
//! passes it through to `resolve_var`, which performs the lookup.

use edgion_resources::resources::edgion_plugins::plugin_configs::{CompiledTemplate, TemplateSegment, TemplateVarType};
use edgion_resources::resources::link_sys::webhook::{
    CustomSource, ListFields, PresencePolicy, ResolvedSecrets, WebhookOutboundRequest,
};

use crate::plugins::runtime::PluginSession;

// ============================================================
// Public compiled types
// ============================================================

/// Pre-compiled outbound request specification.
///
/// Built once at resource-load time from a [`WebhookOutboundRequest`].
/// The `build_request` method evaluates the compiled templates at call time.
#[derive(Debug, Clone)]
pub struct CompiledOutboundRequest {
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

/// Compiled allow-listed fields (origin + custom).
#[derive(Debug, Clone)]
pub struct CompiledListFields {
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
    /// Field name in the outbound webhook request.
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

impl CompiledOutboundRequest {
    /// Compile a [`WebhookOutboundRequest`] into a `CompiledOutboundRequest`.
    ///
    /// Templates are parsed once at load time. `${secretRef:...}` segments are
    /// stored as `Template` entries and resolved at call time from the
    /// `resolved_secrets` map passed to `build_request`.
    ///
    /// Returns `Err` if any template string fails to parse.
    pub fn from_spec(spec: &WebhookOutboundRequest) -> Result<Self, String> {
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

        Ok(CompiledOutboundRequest {
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
    /// * `base_url`      — webhook service base URL (`scheme://host[:port]`).
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
        let mut url = reqwest::Url::parse(&url_str).map_err(|e| format!("invalid webhook URL {:?}: {}", url_str, e))?;

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
        let mut header_pairs: Vec<(String, String)> = Vec::new();
        collect_list_fields(&self.headers, session, FieldKind::Header, &mut header_pairs, resolved)?;
        for (k, v) in header_pairs {
            match (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(&v),
            ) {
                (Ok(name), Ok(val)) => {
                    req.headers_mut().insert(name, val);
                }
                _ => {
                    tracing::warn!(
                        header = %k,
                        "webhook compiled_request: skipping header with invalid name or value"
                    );
                }
            }
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
            match reqwest::header::HeaderValue::from_str(&cookie_str) {
                Ok(val) => {
                    req.headers_mut().insert(reqwest::header::COOKIE, val);
                }
                Err(_) => {
                    tracing::warn!("webhook compiled_request: skipping Cookie header with invalid value");
                }
            }
        }

        Ok(req)
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

    Ok(CompiledListFields { origin, custom })
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
        TemplateVarType::MatchedHttpRoute => {
            session.matched_route_meta(crate::plugins::runtime::traits::session::RouteProto::Http, var_name?)
        }
        TemplateVarType::MatchedGrpcRoute => {
            session.matched_route_meta(crate::plugins::runtime::traits::session::RouteProto::Grpc, var_name?)
        }
        // Resolved at call time from the controller-carried map (keyed by the
        // raw var_name "ns/name:key"). Absent → None (the readiness gate ensures
        // a served webhook never has an absent ref).
        TemplateVarType::SecretRef => resolved.and_then(|r| r.0.get(var_name?).cloned()),
    }
}

/// Collect resolved field key-value pairs from `fields` into `output`.
///
/// Origin fields are read from the incoming request via the session.
/// Custom fields are evaluated from their compiled source.
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
                PresencePolicy::SkipIfMissing => {
                    // Silently omit.
                }
                PresencePolicy::WarnIfMissing => {
                    tracing::warn!(
                        field = %origin.name,
                        "webhook outbound: origin field missing (warnIfMissing), omitting"
                    );
                    // Omit without error.
                }
                PresencePolicy::Required => {
                    return Err(format!(
                        "required webhook field {:?} is missing from incoming request",
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
    use edgion_resources::resources::link_sys::webhook::{
        CustomField, CustomSource, ListFields, MethodSpec, OriginField, PathSpec, WebhookOutboundRequest,
    };

    // Build a minimal session mock that covers the path/method fallback paths.
    // Only `get_method` and `get_request_path` are set up here; callers add
    // extra expectations for origin-field / template tests.
    fn make_session(method: &str, path: &str) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const(method.to_string());
        session.expect_get_request_path().return_const(path.to_string());
        session
    }

    // ── Test 1: origin header present ──────────────────────────────────────
    #[test]
    fn origin_header_present_is_forwarded() {
        let spec = WebhookOutboundRequest {
            headers: ListFields {
                origin: vec![OriginField {
                    name: "X-Tenant-Id".to_string(),
                    presence: PresencePolicy::SkipIfMissing,
                }],
                custom: vec![],
            },
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();

        let mut session = make_session("GET", "/api/v1");
        session
            .expect_header_value()
            .withf(|name| name == "X-Tenant-Id")
            .return_const(Some("tenant-123".to_string()));

        let req = compiled.build_request(&session, None, "http://svc:8080", None).unwrap();
        let header = req.headers().get("x-tenant-id").and_then(|v| v.to_str().ok());
        assert_eq!(header, Some("tenant-123"));
    }

    // ── Test 2: origin field missing + skipIfMissing ───────────────────────
    #[test]
    fn origin_header_missing_skip_if_missing_omits_field() {
        let spec = WebhookOutboundRequest {
            headers: ListFields {
                origin: vec![OriginField {
                    name: "X-Optional".to_string(),
                    presence: PresencePolicy::SkipIfMissing,
                }],
                custom: vec![],
            },
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();

        let mut session = make_session("GET", "/api/v1");
        session
            .expect_header_value()
            .withf(|name| name == "X-Optional")
            .return_const(None::<String>);

        let req = compiled.build_request(&session, None, "http://svc:8080", None).unwrap();
        assert!(req.headers().get("x-optional").is_none());
    }

    // ── Test 3: origin field missing + required => error ──────────────────
    #[test]
    fn origin_header_missing_required_returns_error() {
        let spec = WebhookOutboundRequest {
            headers: ListFields {
                origin: vec![OriginField {
                    name: "X-Required-Header".to_string(),
                    presence: PresencePolicy::Required,
                }],
                custom: vec![],
            },
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();

        let mut session = make_session("GET", "/api/v1");
        session
            .expect_header_value()
            .withf(|name| name == "X-Required-Header")
            .return_const(None::<String>);

        let result = compiled.build_request(&session, None, "http://svc:8080", None);
        assert!(result.is_err(), "expected Err for missing required field");
        let err = result.unwrap_err();
        assert!(
            err.contains("X-Required-Header"),
            "error should name the missing field, got: {err}"
        );
    }

    // ── Test 4: custom literal value ───────────────────────────────────────
    #[test]
    fn custom_literal_value_injected_into_query() {
        let spec = WebhookOutboundRequest {
            args: ListFields {
                origin: vec![],
                custom: vec![CustomField {
                    name: "version".to_string(),
                    source: CustomSource::Value("v2".to_string()),
                }],
            },
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();
        let session = make_session("GET", "/api");

        let req = compiled.build_request(&session, None, "http://svc:8080", None).unwrap();
        let pairs: Vec<(String, String)> = req
            .url()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let version = pairs.iter().find(|(k, _)| k == "version").map(|(_, v)| v.as_str());
        assert_eq!(version, Some("v2"));
    }

    // ── Test 5: custom template with ctx variables ─────────────────────────
    #[test]
    fn custom_template_evaluated_at_call_time() {
        let spec = WebhookOutboundRequest {
            args: ListFields {
                origin: vec![],
                custom: vec![CustomField {
                    name: "key".to_string(),
                    source: CustomSource::Template("${ctx:tenant}_${ctx:region}".to_string()),
                }],
            },
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();

        let mut session = make_session("GET", "/api");
        session
            .expect_get_ctx_var()
            .withf(|k| k == "tenant")
            .returning(|_| Some("acme".to_string()));
        session
            .expect_get_ctx_var()
            .withf(|k| k == "region")
            .returning(|_| Some("us-east".to_string()));

        let req = compiled.build_request(&session, None, "http://svc:8080", None).unwrap();
        let pairs: Vec<(String, String)> = req
            .url()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let key_value = pairs.iter().find(|(k, _)| k == "key").map(|(_, v)| v.clone());
        assert_eq!(key_value.as_deref(), Some("acme_us-east"));
    }

    // ── Test 6: path_override allowed ─────────────────────────────────────
    #[test]
    fn path_override_allowed_uses_override_path() {
        let spec = WebhookOutboundRequest {
            path: Some(PathSpec {
                template: "/default-path".to_string(),
                allow_override: true,
            }),
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();
        let session = make_session("GET", "/ignored");

        let req = compiled
            .build_request(&session, Some("/override-path"), "http://svc:8080", None)
            .unwrap();
        assert_eq!(req.url().path(), "/override-path");
    }

    // ── Test 7: path_override not allowed ─────────────────────────────────
    #[test]
    fn path_override_disallowed_uses_template_path() {
        let spec = WebhookOutboundRequest {
            path: Some(PathSpec {
                template: "/fixed-path".to_string(),
                allow_override: false,
            }),
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();
        let session = make_session("GET", "/ignored");

        let req = compiled
            .build_request(&session, Some("/attempted-override"), "http://svc:8080", None)
            .unwrap();
        assert_eq!(req.url().path(), "/fixed-path");
    }

    // ── Test 8: no path spec => frozen incoming path ───────────────────────
    #[test]
    fn no_path_spec_uses_frozen_request_path() {
        let spec = WebhookOutboundRequest::default();
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();
        let session = make_session("GET", "/frozen/path");

        let req = compiled.build_request(&session, None, "http://svc:8080", None).unwrap();
        assert_eq!(req.url().path(), "/frozen/path");
    }

    // ── Test 9: no method spec => frozen incoming method ──────────────────
    #[test]
    fn no_method_spec_uses_frozen_incoming_method() {
        let spec = WebhookOutboundRequest {
            method: Some(MethodSpec {
                template: "DELETE".to_string(),
            }),
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();
        let session = make_session("POST", "/api/resource");

        let req = compiled.build_request(&session, None, "http://svc:8080", None).unwrap();
        assert_eq!(req.method().as_str(), "DELETE");

        // Also verify the true "frozen method" case: no method spec at all.
        let spec_no_method = WebhookOutboundRequest::default();
        let compiled2 = CompiledOutboundRequest::from_spec(&spec_no_method).unwrap();
        let session2 = make_session("PATCH", "/api/resource");
        let req2 = compiled2
            .build_request(&session2, None, "http://svc:8080", None)
            .unwrap();
        assert_eq!(req2.method().as_str(), "PATCH");
    }

    // ── Secret injected via ${secretRef:...} resolved at call time from the map.
    #[test]
    fn custom_secret_ref_resolved_at_call_time() {
        use edgion_resources::resources::link_sys::webhook::ResolvedSecrets;
        let spec = WebhookOutboundRequest {
            headers: ListFields {
                origin: vec![],
                custom: vec![CustomField {
                    name: "Authorization".to_string(),
                    source: CustomSource::Template("Bearer ${secretRef:default/my-secret:token}".to_string()),
                }],
            },
            ..WebhookOutboundRequest::default()
        };
        let compiled = CompiledOutboundRequest::from_spec(&spec).unwrap();

        let mut map = std::collections::BTreeMap::new();
        map.insert("default/my-secret:token".to_string(), "supersecret".to_string());
        let resolved = ResolvedSecrets(map);

        let session = make_session("GET", "/api");
        let req = compiled
            .build_request(&session, None, "http://svc:8080", Some(&resolved))
            .unwrap();
        let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok());
        assert_eq!(auth, Some("Bearer supersecret"));
    }
}
