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

use std::borrow::Cow;

use edgion_resources::resources::common::external_http::ResolvedSecrets;
use edgion_resources::resources::common::outbound_request::{
    BodySpec, CustomSource, ListFields, OutboundRequestSpec, PresencePolicy,
};
use edgion_resources::resources::common::request_template::CompiledTemplate;

use crate::link_sys::template::{
    render_template_in, EscapeMode, RenderPolicy, RenderedTemplate, SecretCapability, TemplateContext,
};
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
        self.build_request_in(
            TemplateContext::Request(session),
            path_override,
            base_url,
            resolved,
            diagnostics,
        )
    }

    /// Build with explicit template and incoming-request read capabilities.
    pub fn build_request_in(
        &self,
        context: TemplateContext<'_>,
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
                        Cow::Borrowed(ov)
                    } else {
                        Cow::Owned(evaluate_template(&path_spec.template, context, resolved)?)
                    }
                } else {
                    Cow::Owned(evaluate_template(&path_spec.template, context, resolved)?)
                }
            }
            // No path spec: use the frozen entry-time path.
            None => Cow::Borrowed(
                context
                    .request_session()
                    .map_err(|error| error.to_string())?
                    .get_request_path(),
            ),
        };

        // ── 2. Determine method ───────────────────────────────────────────
        let final_method = match &self.method {
            Some(method_tpl) => Cow::Owned(evaluate_template(method_tpl, context, resolved)?),
            None => Cow::Borrowed(
                context
                    .request_session()
                    .map_err(|error| error.to_string())?
                    .get_request_method()
                    .ok_or_else(|| "missing incoming request method".to_string())?,
            ),
        };

        // ── 3. Collect query args ─────────────────────────────────────────
        let mut query_pairs: Vec<CollectedField> = Vec::new();
        collect_list_fields(
            &self.args,
            context,
            FieldKind::Query,
            &mut query_pairs,
            resolved,
            diagnostics,
        )?;

        // ── 4. Build URL ──────────────────────────────────────────────────
        let url_base = base_url.trim_end_matches('/');
        let slash = if final_path.starts_with('/') { "" } else { "/" };
        let url_str = format!("{}{}{}", url_base, slash, final_path);
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
        *req.headers_mut() = self.collect_headers_in(context, resolved, diagnostics)?;

        // ── 8. Add cookies as a single Cookie header ──────────────────────
        let mut cookie_pairs: Vec<CollectedField> = Vec::new();
        collect_list_fields(
            &self.cookies,
            context,
            FieldKind::Cookie,
            &mut cookie_pairs,
            resolved,
            diagnostics,
        )?;
        if !cookie_pairs.is_empty() {
            let mut cookie_str = String::new();
            for (index, field) in cookie_pairs.iter().enumerate() {
                if index > 0 {
                    cookie_str.push_str("; ");
                }
                cookie_str.push_str(&field.name);
                cookie_str.push('=');
                cookie_str.push_str(&field.value);
            }
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
                    let rendered = render_outbound(tpl, context, resolved, EscapeMode::JsonString)?.into_string();
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
                            CompiledCustomFieldSource::Template(tpl) => evaluate_template(tpl, context, resolved)?,
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
                    let rendered = evaluate_template(tpl, context, resolved)?;
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
        self.collect_headers_in(TemplateContext::Request(session), resolved, diagnostics)
    }

    /// Collect headers using only the supplied request or trusted capability.
    pub fn collect_headers_in(
        &self,
        context: TemplateContext<'_>,
        resolved: Option<&ResolvedSecrets>,
        diagnostics: &mut OutboundRequestDiagnostics,
    ) -> Result<reqwest::header::HeaderMap, String> {
        let mut result = reqwest::header::HeaderMap::new();

        // forward_all: add all session headers minus hop-by-hop (RFC 9110 §7.6.1).
        if self.headers.forward_all {
            let session = context.request_session().map_err(|error| error.to_string())?;
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
            context,
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
    context: TemplateContext<'_>,
    resolved: Option<&ResolvedSecrets>,
    escape: EscapeMode,
) -> Result<RenderedTemplate, String> {
    render_template_in(
        tpl,
        context,
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
    context: TemplateContext<'_>,
    resolved: Option<&ResolvedSecrets>,
) -> Result<String, String> {
    render_outbound(tpl, context, resolved, EscapeMode::None).map(RenderedTemplate::into_string)
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
    context: TemplateContext<'_>,
    kind: FieldKind,
    output: &mut Vec<CollectedField>,
    resolved: Option<&ResolvedSecrets>,
    diagnostics: &mut OutboundRequestDiagnostics,
) -> Result<(), String> {
    if fields.forward_all {
        context.request_session().map_err(|error| error.to_string())?;
    }
    // Origin fields: forwarded from the incoming request.
    for origin in &fields.origin {
        let value = get_session_field(
            context.request_session().map_err(|error| error.to_string())?,
            &kind,
            &origin.name,
        );
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
                render_outbound(tpl, context, resolved, EscapeMode::None)?.into_parts()
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

#[cfg(test)]
include!("../../../../../tests/support/plugins/http/common/outbound/request_test/625_function_item.rs");

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/common/outbound/request_test.rs"]
mod tests;
