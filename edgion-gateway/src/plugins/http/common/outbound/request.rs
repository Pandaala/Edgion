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
//! passes it to the canonical template renderer with an explicit Secret capability.

use edgion_resources::resources::common::external_http::ResolvedSecrets;
use edgion_resources::resources::common::outbound_request::{
    BodySpec, CustomSource, ListFields, OutboundRequestSpec, PresencePolicy,
};
use edgion_resources::resources::common::request_template::CompiledTemplate;

use crate::link_sys::template::{render_template, EscapeMode, RenderPolicy, RenderedTemplate, SecretCapability};
use crate::plugins::http::common::http_client::{connection_listed_hop_by_hop, is_hop_by_hop_or_listed};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt};

/// Value-free evidence collected during one logical outbound request build.
///
/// A single bit bounds storage independently of configured field count. The
/// caller owns it outside the call future so build/transport failures do not
/// discard warnings already observed. Retries reuse the built request.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OutboundRequestDiagnostics {
    missing_origin: bool,
}

impl OutboundRequestDiagnostics {
    /// Consume the evidence at the owning plugin's logging boundary, at most
    /// once per logical call. Never records field names, values, or errors.
    pub fn emit(self, plugin_log: &mut PluginLog) {
        if self.missing_origin {
            plugin_log.push("warn outbound field missing");
        }
    }
}

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
pub(crate) fn sync_outbound_body_framing(req: &mut reqwest::Request) -> Result<(), String> {
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
        if let Some(error) = spec.validation_error() {
            return Err(error);
        }
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
    ///   and template variable resolution.
    /// * `path_override` — caller-supplied path suffix; honoured only when
    ///   the compiled path spec has `allow_override = true`.
    ///   Ignored (not an error) when override is disallowed.
    /// * `base_url`      — service base URL (`scheme://host[:port]`).
    ///   A trailing `/` is stripped before appending the path.
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
        diagnostics: &mut OutboundRequestDiagnostics,
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
        let mut query_pairs: Vec<CollectedField> = Vec::new();
        collect_list_fields(
            &self.args,
            session,
            FieldKind::Query,
            &mut query_pairs,
            resolved,
            diagnostics,
        )?;

        // ── 4. Build URL ──────────────────────────────────────────────────
        let url_base = base_url.trim_end_matches('/');
        let path_part = if final_path.starts_with('/') {
            final_path.clone()
        } else {
            format!("/{}", final_path)
        };
        let url_str = format!("{}{}", url_base, path_part);
        let mut url = reqwest::Url::parse(&url_str).map_err(|_| "invalid outbound URL".to_string())?;

        if !query_pairs.is_empty() {
            let mut qp = url.query_pairs_mut();
            for field in &query_pairs {
                qp.append_pair(&field.name, &field.value);
            }
        }

        // ── 5. Parse HTTP method ──────────────────────────────────────────
        let method = reqwest::Method::from_bytes(final_method.as_bytes())
            .map_err(|_| "invalid outbound HTTP method".to_string())?;

        // ── 6. Create request ─────────────────────────────────────────────
        let mut req = reqwest::Request::new(method, url);

        // ── 7. Add headers ────────────────────────────────────────────────
        // Use collect_headers so that forward_all semantics are honoured:
        // when forward_all=true all non-hop-by-hop headers from the incoming
        // request are forwarded, and explicit origin/custom entries are applied
        // on top (possibly overriding). When forward_all=false only the
        // explicit origin/custom entries are sent.
        *req.headers_mut() = self.collect_headers(session, resolved, diagnostics)?;

        // ── 8. Add cookies as a single Cookie header ──────────────────────
        let mut cookie_pairs: Vec<CollectedField> = Vec::new();
        collect_list_fields(
            &self.cookies,
            session,
            FieldKind::Cookie,
            &mut cookie_pairs,
            resolved,
            diagnostics,
        )?;
        if !cookie_pairs.is_empty() {
            let cookie_str = cookie_pairs
                .iter()
                .map(|field| format!("{}={}", field.name, field.value))
                .collect::<Vec<_>>()
                .join("; ");
            if let Ok(mut val) = reqwest::header::HeaderValue::from_str(&cookie_str) {
                val.set_sensitive(true);
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
                    let rendered = render_outbound(tpl, session, resolved, EscapeMode::JsonString)?.into_string();
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
    /// Returns `Ok` with a multi-value header map on success, or `Err` if a
    /// `Required` origin header is absent from the incoming request.
    pub fn collect_headers(
        &self,
        session: &dyn PluginSession,
        resolved: Option<&ResolvedSecrets>,
        diagnostics: &mut OutboundRequestDiagnostics,
    ) -> Result<reqwest::header::HeaderMap, String> {
        let mut result = reqwest::header::HeaderMap::new();

        // forward_all: add all session headers minus hop-by-hop (RFC 9110 §7.6.1).
        if self.headers.forward_all {
            let conn_val = session.header_str("connection").unwrap_or_default();
            let dynamic = connection_listed_hop_by_hop(conn_val);
            for (name, value) in session.request_header_map() {
                // Preserve the owned accessor's behavior: a header whose value is
                // not valid UTF-8 is skipped entirely.
                let Ok(_) = std::str::from_utf8(value.as_bytes()) else {
                    continue;
                };
                if !is_hop_by_hop_or_listed(name.as_str(), &dynamic)
                    && !is_body_framing_header(name.as_str())
                    && !is_hop_owned_expectation_header(name.as_str())
                {
                    result.append(name.clone(), value.clone());
                }
            }
        }

        // Apply explicit origin + custom fields (may override forward_all entries).
        // Propagate Required-field errors rather than swallowing them.
        // Pass resolved so that ${secretRef:...} templates in custom fields resolve.
        let mut pairs: Vec<CollectedField> = Vec::new();
        collect_list_fields(
            &self.headers,
            session,
            FieldKind::Header,
            &mut pairs,
            resolved,
            diagnostics,
        )?;
        for field in pairs {
            if let (Ok(name), Ok(mut value)) = (
                reqwest::header::HeaderName::from_bytes(field.name.as_bytes()),
                reqwest::header::HeaderValue::from_str(&field.value),
            ) {
                // Provenance survives assembly; protect Secrets before storage/transport.
                value.set_sensitive(field.sensitive);
                result.insert(name, value);
            }
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

/// Outbound surfaces borrow only the controller-carried resolved Secret map.
fn render_outbound(
    tpl: &CompiledTemplate,
    session: &dyn PluginSession,
    resolved: Option<&ResolvedSecrets>,
    escape: EscapeMode,
) -> Result<RenderedTemplate, String> {
    render_template(
        tpl,
        session,
        RenderPolicy {
            escape,
            ..RenderPolicy::EMPTY
        },
        SecretCapability::Resolved(resolved),
    )
    .map_err(|error| error.to_string())
}

/// Value-only sinks (path, query and body) consume provenance after rendering.
fn evaluate_template(
    tpl: &CompiledTemplate,
    session: &dyn PluginSession,
    resolved: Option<&ResolvedSecrets>,
) -> Result<String, String> {
    render_outbound(tpl, session, resolved, EscapeMode::None).map(RenderedTemplate::into_string)
}

/// No Debug implementation: values can contain credentials.
struct CollectedField {
    name: String,
    value: String,
    sensitive: bool,
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
    output: &mut Vec<CollectedField>,
    resolved: Option<&ResolvedSecrets>,
    diagnostics: &mut OutboundRequestDiagnostics,
) -> Result<(), String> {
    // Origin fields: forwarded from the incoming request.
    for origin in &fields.origin {
        let value = get_session_field(session, &kind, &origin.name);
        match value {
            Some(value) => output.push(CollectedField {
                name: origin.name.clone(),
                value,
                sensitive: true,
            }),
            None => match origin.presence {
                PresencePolicy::SkipIfMissing => {}
                PresencePolicy::WarnIfMissing => {
                    diagnostics.missing_origin = true;
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
        let (value, sensitive) = match &custom.source {
            // Preserve the existing conservative classification of static credentials.
            CompiledCustomFieldSource::Literal(v) => (v.clone(), true),
            CompiledCustomFieldSource::Template(tpl) => {
                render_outbound(tpl, session, resolved, EscapeMode::None)?.into_parts()
            }
        };
        output.push(CollectedField {
            name: custom.name.clone(),
            value,
            sensitive,
        });
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

/// Reach a deterministic build failure after inspecting the header policy,
/// without opening a socket. Shared by consumer wiring regression tests.
#[cfg(test)]
pub(crate) fn test_presence_failure_request(presence: PresencePolicy) -> CompiledRequest {
    use edgion_resources::resources::common::outbound_request::{MethodSpec, OriginField, PathSpec};
    CompiledRequest::from_spec(&OutboundRequestSpec {
        method: Some(MethodSpec { template: "GET".into() }),
        path: Some(PathSpec {
            template: "/verify".into(),
            allow_override: false,
        }),
        headers: ListFields {
            origin: vec![OriginField {
                name: "x-private-metadata".into(),
                presence,
            }],
            ..Default::default()
        },
        cookies: ListFields {
            origin: vec![OriginField {
                name: "private-session".into(),
                presence: PresencePolicy::Required,
            }],
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
    use edgion_resources::resources::common::outbound_request::ListFields;

    #[test]
    fn from_spec_rejects_malformed_explicit_header_name() {
        use edgion_resources::resources::common::outbound_request::{CustomField, CustomSource};

        let marker = "private malformed header";
        let spec = OutboundRequestSpec {
            headers: ListFields {
                custom: vec![CustomField {
                    name: marker.to_string(),
                    source: CustomSource::Value("value".to_string()),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let error = CompiledRequest::from_spec(&spec).expect_err("malformed header name must fail compilation");
        assert_eq!(error, "request.headers.custom[0].name must be a valid HTTP header name");
        assert!(!error.contains(marker));
    }

    #[test]
    fn presence_policies_cover_missing_and_present_fields() {
        use edgion_resources::resources::common::outbound_request::{MethodSpec, OriginField, PathSpec};
        for kind in [FieldKind::Header, FieldKind::Query, FieldKind::Cookie] {
            let forward_all_values: &[bool] = if matches!(kind, FieldKind::Header) {
                &[false, true]
            } else {
                &[false]
            };
            for &forward_all in forward_all_values {
                for presence in [
                    PresencePolicy::SkipIfMissing,
                    PresencePolicy::WarnIfMissing,
                    PresencePolicy::Required,
                ] {
                    for present in [false, true] {
                        let mut spec = OutboundRequestSpec {
                            method: Some(MethodSpec { template: "GET".into() }),
                            path: Some(PathSpec {
                                template: "/verify".into(),
                                allow_override: false,
                            }),
                            ..Default::default()
                        };
                        let fields = match kind {
                            FieldKind::Header => &mut spec.headers,
                            FieldKind::Query => &mut spec.args,
                            FieldKind::Cookie => &mut spec.cookies,
                        };
                        fields.forward_all = forward_all;
                        fields.origin.push(OriginField {
                            name: "x-source".into(),
                            presence,
                        });
                        let compiled = CompiledRequest::from_spec(&spec).unwrap();
                        let mut session = MockPluginSession::new();
                        setup_header_mocks(
                            &mut session,
                            if present { &[("x-source", "private-value")] } else { &[] },
                        );
                        session
                            .expect_get_query_param()
                            .returning(move |_| present.then(|| "private-value".into()));
                        session
                            .expect_get_cookie()
                            .returning(move |_| present.then(|| "private-value".into()));
                        let mut diagnostics = OutboundRequestDiagnostics::default();
                        let result =
                            compiled.build_request(&session, None, "http://example.test", None, &mut diagnostics);
                        assert_eq!(
                            result.is_err(),
                            !present && matches!(presence, PresencePolicy::Required)
                        );
                        if let Ok(request) = result {
                            let forwarded = match kind {
                                FieldKind::Header => request
                                    .headers()
                                    .get("x-source")
                                    .map(|v| v.to_str().unwrap().to_owned()),
                                FieldKind::Query => request
                                    .url()
                                    .query_pairs()
                                    .find(|(k, _)| k == "x-source")
                                    .map(|(_, v)| v.into_owned()),
                                FieldKind::Cookie => {
                                    request.headers().get("cookie").map(|v| v.to_str().unwrap().to_owned())
                                }
                            };
                            assert_eq!(forwarded.is_some(), present);
                            if let Some(value) = forwarded {
                                assert!(value.contains("private-value"));
                            }
                        }
                        let mut log = PluginLog::new("test");
                        diagnostics.emit(&mut log);
                        assert_eq!(
                            log.render_for_test(),
                            if !present && matches!(presence, PresencePolicy::WarnIfMissing) {
                                "warn outbound field missing"
                            } else {
                                ""
                            }
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn missing_warning_is_bounded_and_survives_later_required_failure() {
        let mut compiled = test_presence_failure_request(PresencePolicy::WarnIfMissing);
        compiled.headers.origin = vec![compiled.headers.origin[0].clone(); 1024];
        compiled.args = compiled.headers.clone();
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[]);
        session.expect_get_query_param().return_const(None::<String>);
        session.expect_get_cookie().return_const(None::<String>);
        let mut diagnostics = OutboundRequestDiagnostics::default();
        assert!(compiled
            .build_request(&session, None, "http://example.test", None, &mut diagnostics)
            .is_err());
        let mut log = PluginLog::new("test");
        diagnostics.emit(&mut log);
        assert_eq!(log.render_for_test(), "warn outbound field missing");
        assert_eq!(std::mem::size_of::<OutboundRequestDiagnostics>(), 1);
    }

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
        // All request headers the session exposes, including hop-by-hop and normal ones.
        setup_header_mocks(
            &mut session,
            &[
                ("content-type", "application/json"),
                ("content-length", "9"),
                ("transfer-encoding", "chunked"),
                ("connection", "X-Custom-Hop"),
                ("keep-alive", "timeout=5"),
                ("x-custom-hop", "remove-this"),
                ("authorization", "Bearer token123"),
            ],
        );

        let result = compiled
            .collect_headers(&session, None, &mut Default::default())
            .unwrap();

        // Non-hop-by-hop headers must be present.
        assert_eq!(
            result.get("content-type").and_then(|value| value.to_str().ok()),
            Some("application/json"),
            "content-type should be forwarded"
        );
        assert_eq!(
            result.get("authorization").and_then(|value| value.to_str().ok()),
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
    fn forward_all_preserves_duplicate_header_values() {
        use edgion_resources::resources::common::outbound_request::{
            CustomField, CustomSource, MethodSpec, OutboundRequestSpec, PathSpec,
        };

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
                custom: vec![CustomField {
                    name: "X-Role".to_string(),
                    source: CustomSource::Value("configured".to_string()),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        setup_header_mocks(
            &mut session,
            &[
                ("x-forwarded-for", "192.0.2.1"),
                ("x-forwarded-for", "198.51.100.2"),
                ("x-role", "incoming-first"),
                ("x-role", "incoming-second"),
            ],
        );

        let req = compiled
            .build_request(&session, None, "http://auth.test", None, &mut Default::default())
            .unwrap();
        let values = req
            .headers()
            .get_all("x-forwarded-for")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["192.0.2.1", "198.51.100.2"]);
        assert_eq!(
            req.headers()
                .get_all("x-role")
                .iter()
                .map(|value| value.to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["configured"]
        );
    }

    #[test]
    fn forward_all_preserves_sensitive_values_and_utf8_filtering() {
        let spec = OutboundRequestSpec {
            headers: ListFields {
                forward_all: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();
        let mut headers = http::HeaderMap::new();
        for bytes in [b"private-first".as_slice(), b"private-second".as_slice()] {
            let mut value = http::HeaderValue::from_bytes(bytes).unwrap();
            value.set_sensitive(true);
            headers.append("x-custom-credential", value);
        }
        headers.insert("x-visible", http::HeaderValue::from_static("public"));
        headers.insert("x-utf8", http::HeaderValue::from_bytes("é".as_bytes()).unwrap());
        headers.insert("x-invalid-utf8", http::HeaderValue::from_bytes(&[0xff]).unwrap());
        headers.insert("connection", http::HeaderValue::from_static("x-private-hop"));
        let mut hop = http::HeaderValue::from_static("hop-secret");
        hop.set_sensitive(true);
        headers.insert("x-private-hop", hop);
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);

        let result = compiled
            .collect_headers(&session, None, &mut Default::default())
            .unwrap();
        let values: Vec<_> = result.get_all("x-custom-credential").iter().collect();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].as_bytes(), b"private-first");
        assert_eq!(values[1].as_bytes(), b"private-second");
        assert!(values.iter().all(|value| value.is_sensitive()));
        assert!(!result["x-visible"].is_sensitive());
        assert_eq!(result["x-utf8"].as_bytes(), "é".as_bytes());
        assert!(!result.contains_key("x-invalid-utf8"));
        assert!(!result.contains_key("connection"));
        assert!(!result.contains_key("x-private-hop"));
        assert!(!format!("{result:?}").contains("private-first"));
    }

    #[test]
    fn rendered_credentials_origin_headers_and_cookies_are_sensitive() {
        use edgion_resources::resources::common::outbound_request::{OriginField, PresencePolicy};
        let mut spec = body_spec(BodySpec::Raw { template: "".into() });
        spec.headers = ListFields {
            origin: vec![OriginField {
                name: "x-identity".into(),
                presence: PresencePolicy::Required,
            }],
            custom: vec![
                CustomField {
                    name: "x-arbitrary-credential".into(),
                    source: CustomSource::Template("Bearer ${secretRef:default/auth:token}".into()),
                },
                CustomField {
                    name: "x-configured".into(),
                    source: CustomSource::Value("fixed-value".into()),
                },
            ],
            ..Default::default()
        };
        spec.cookies = ListFields {
            custom: vec![CustomField {
                name: "session".into(),
                source: CustomSource::Template("${secretRef:default/auth:token}".into()),
            }],
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "x-identity")
            .return_const(Some("private-user".to_owned()));
        let resolved = ResolvedSecrets(std::collections::BTreeMap::from([(
            "default/auth:token".into(),
            "private-token".into(),
        )]));
        let request = compiled
            .build_request(
                &session,
                None,
                "http://upstream.test",
                Some(&resolved),
                &mut Default::default(),
            )
            .unwrap();
        for (name, expected) in [
            ("x-identity", "private-user"),
            ("x-arbitrary-credential", "Bearer private-token"),
            ("x-configured", "fixed-value"),
            ("cookie", "session=private-token"),
        ] {
            assert_eq!(request.headers()[name].as_bytes(), expected.as_bytes());
            assert!(request.headers()[name].is_sensitive());
        }
        assert!(!format!("{:?}", request.headers()).contains("private-token"));
    }

    #[test]
    fn template_header_provenance_survives_header_only_and_request_clones() {
        let mut spec = body_spec(BodySpec::Raw { template: "".into() });
        spec.headers.custom = vec![
            CustomField {
                name: "x-private".into(),
                source: CustomSource::Template("prefix-${secretRef:ns/token:key}-suffix".into()),
            },
            CustomField {
                name: "x-public".into(),
                source: CustomSource::Template("prefix-${ctx:name}-suffix".into()),
            },
        ];
        let compiled = CompiledRequest::from_spec(&spec).unwrap();
        let mut session = MockPluginSession::new();
        session.expect_get_ctx_var().return_const(Some("public".to_string()));
        let secret = ResolvedSecrets(std::collections::BTreeMap::from([(
            "ns/token:key".to_string(),
            "private".to_string(),
        )]));
        for resolved in [Some(&secret), None] {
            let headers = compiled
                .collect_headers(&session, resolved, &mut Default::default())
                .unwrap();
            let expected = if resolved.is_some() {
                "prefix-private-suffix"
            } else {
                "prefix--suffix"
            };
            assert_eq!(headers["x-private"], expected);
            assert!(headers["x-private"].is_sensitive());
            assert_eq!(headers["x-public"], "prefix-public-suffix");
            assert!(!headers["x-public"].is_sensitive());
            let request = compiled
                .build_request(
                    &session,
                    None,
                    "http://upstream.test",
                    resolved,
                    &mut Default::default(),
                )
                .unwrap();
            let cloned = request.try_clone().unwrap();
            assert!(cloned.headers()["x-private"].is_sensitive());
            assert!(!cloned.headers()["x-public"].is_sensitive());
            assert!(!format!("{:?}", cloned.headers()).contains("prefix-private-suffix"));
        }
    }

    #[test]
    fn rendered_url_and_method_errors_do_not_disclose_template_values() {
        use edgion_resources::resources::common::outbound_request::MethodSpec;
        let mut spec = body_spec(BodySpec::Raw { template: "".into() });
        spec.method = Some(MethodSpec {
            template: "${secretRef:ns/token:key}".into(),
        });
        let secret = ResolvedSecrets(std::collections::BTreeMap::from([(
            "ns/token:key".to_string(),
            "sentinel private".to_string(),
        )]));
        let compiled = CompiledRequest::from_spec(&spec).unwrap();
        let session = MockPluginSession::new();
        let error = compiled
            .build_request(
                &session,
                None,
                "http://upstream.test",
                Some(&secret),
                &mut Default::default(),
            )
            .unwrap_err();
        assert_eq!(error, "invalid outbound HTTP method");
        let error = compiled
            .build_request(
                &session,
                None,
                "http://[sentinel-private",
                Some(&secret),
                &mut Default::default(),
            )
            .unwrap_err();
        assert_eq!(error, "invalid outbound URL");
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
        setup_header_mocks(
            &mut session,
            &[("authorization", "Bearer valid-token"), ("content-length", "9")],
        );

        let req = compiled
            .build_request(&session, None, "http://auth.test", None, &mut Default::default())
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
        // The session headers do not include X-Required.
        setup_header_mocks(&mut session, &[("content-type", "text/plain")]);

        let result = compiled.collect_headers(&session, None, &mut Default::default());
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
        setup_header_mocks(
            &mut session,
            &[("expect", "100-continue"), ("authorization", "Bearer token123")],
        );

        let result = compiled
            .collect_headers(&session, None, &mut Default::default())
            .unwrap();

        assert!(
            !result.contains_key("expect"),
            "expect states an expectation owned by the inbound hop"
        );
        assert_eq!(
            result.get("authorization").and_then(|value| value.to_str().ok()),
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
        // Deliberately NOT `setup_header_mocks`: this test needs the two lookup
        // paths to disagree so the assertion can tell which branch produced the
        // entry. The forward_all sweep reads the borrowed map; the explicitly
        // listed origin field renders through `header_value`.
        let mut map = http::HeaderMap::new();
        map.append(
            http::HeaderName::from_static("expect"),
            http::HeaderValue::from_static("100-continue-from-sweep"),
        );
        session.expect_request_header_map().return_const(map);
        session.expect_header_value().returning(|name| {
            name.eq_ignore_ascii_case("expect")
                .then(|| "100-continue-via-origin".to_string())
        });

        let result = compiled
            .collect_headers(&session, None, &mut Default::default())
            .unwrap();

        assert_eq!(
            result.get("expect").and_then(|value| value.to_str().ok()),
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
            template: r#"{"user": "${ctx:user}", "source": "edgion"}"#.to_string(),
        });
        let compiled = CompiledRequest::from_spec(&spec).unwrap();

        let mut session = MockPluginSession::new();
        // Value contains a quote, a backslash, and a newline — all must be escaped.
        session
            .expect_get_ctx_var()
            .withf(|n| n == "user")
            .return_const(Some("ali\"ce\\bob\nline2".to_string()));

        let req = compiled
            .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
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
            Some(body.len().to_string().as_str())
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
            .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
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
            .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
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
        setup_header_mocks(&mut session, &[("content-type", "text/plain")]);

        let req = compiled
            .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
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
