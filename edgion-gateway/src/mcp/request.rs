//! Activation-gated MCP 2026-07-28 request validation.
//!
//! WP10-12 installs the public request-stage call. Until then this complete
//! internal API is intentionally reachable only from focused tests and later
//! WP10 subtasks.

#![allow(dead_code)]

use std::sync::Arc;

use base64::Engine as _;
use edgion_resources::http_origin::ExactCorsOriginSet;
use edgion_resources::resources::edgion_plugins::{parse_json_instance, StrictJsonError};
use edgion_resources::resources::{validate_mcp_http_route_rule, HTTPBackendRef, HTTPRouteFilterType};
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use pingora_proxy::Session;
use serde_json::{json, Map, Value};

use crate::ai::{EdgionBackendSnapshot, RuntimeEdgionBackend, RuntimeMcpBackend};
use crate::backends::policy::backend_tls::BackendTLSPolicySnapshot;
use crate::backends::policy::backend_traffic::BackendTrafficPolicySnapshot;
use crate::backends::SelectedMcpBackend;
use crate::plugins::runtime::traits::session::BodyReadError;
use crate::plugins::runtime::traits::PluginSession;
use crate::plugins::runtime::PingoraSessionAdapter;
use crate::plugins::runtime::{McpCorsEvidence, PluginRuntime};
use crate::routes::http::match_unit::{CompiledBackend, CompiledBackendTarget};
use crate::EdgionHttpContext;

pub(crate) const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
pub(crate) const MCP_MAX_BODY_BYTES: u64 = 1024 * 1024;
const MAX_RELEVANT_HEADER_OCCURRENCES: usize = 64;
const MAX_RELEVANT_HEADER_BYTES: usize = 32 * 1024;
const MAX_PARAM_HEADER_OCCURRENCES: usize = 32;
const MAX_DECODED_HEADER_VALUE_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpMethod {
    ServerDiscover,
    ToolsList,
    ToolsCall,
}

impl McpMethod {
    pub(crate) fn as_wire_name(self) -> &'static str {
        match self {
            Self::ServerDiscover => "server/discover",
            Self::ToolsList => "tools/list",
            Self::ToolsCall => "tools/call",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct McpParamHeader {
    pub name: HeaderName,
    pub value: HeaderValue,
}

#[derive(Clone)]
pub(crate) struct McpRequestPlan {
    pub method: McpMethod,
    pub correlation_id: Value,
    pub name: Option<String>,
    pub name_header: Option<HeaderValue>,
    pub params: Map<String, Value>,
    pub client_capabilities: Map<String, Value>,
    pub passthrough_param_headers: Vec<McpParamHeader>,
    pub body_generation: u64,
    pub backend: Arc<RuntimeMcpBackend>,
    pub backend_snapshot: EdgionBackendSnapshot,
    pub traffic_policy_snapshot: BackendTrafficPolicySnapshot,
    pub tls_policy_snapshot: BackendTLSPolicySnapshot,
    selection: McpSelectionState,
}

#[derive(Clone)]
enum McpSelectionState {
    Validated,
    Selected(SelectedMcpBackend),
}

impl McpRequestPlan {
    pub(crate) fn body_generation_matches(&self, ctx: &EdgionHttpContext) -> bool {
        ctx.request_body.generation() == Some(self.body_generation)
    }

    pub(crate) fn selected(&self) -> Option<&SelectedMcpBackend> {
        match &self.selection {
            McpSelectionState::Validated => None,
            McpSelectionState::Selected(selected) => Some(selected),
        }
    }

    pub(crate) fn bind_selected(&mut self, selected: SelectedMcpBackend) -> Result<(), &'static str> {
        if !matches!(self.selection, McpSelectionState::Validated) {
            return Err("MCP backend was already selected");
        }
        if !Arc::ptr_eq(&self.backend, &selected.backend)
            || selected.backend_snapshot_generation != self.backend_snapshot.generation()
            || selected.traffic_policy_snapshot_generation != self.traffic_policy_snapshot.generation()
            || selected.tls_policy_snapshot_generation != self.tls_policy_snapshot.generation()
        {
            return Err("MCP selected backend does not match the request snapshots");
        }
        self.selection = McpSelectionState::Selected(selected);
        Ok(())
    }

    pub(crate) fn selected_matches_context(&self, ctx: &EdgionHttpContext) -> bool {
        let Some(plan_selected) = self.selected() else {
            return false;
        };
        let Some(crate::backends::SelectedLogicalBackend::Mcp(context_selected)) = ctx
            .selected_backend
            .as_ref()
            .and_then(crate::backends::SelectedHttpBackend::routed)
        else {
            return false;
        };
        plan_selected.logical().slot() == context_selected.logical().slot()
            && Arc::ptr_eq(&plan_selected.backend, &context_selected.backend)
            && plan_selected.backend_snapshot_generation == context_selected.backend_snapshot_generation
            && plan_selected.traffic_policy_snapshot_generation == context_selected.traffic_policy_snapshot_generation
            && plan_selected.tls_policy_snapshot_generation == context_selected.tls_policy_snapshot_generation
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct McpLocalError {
    pub status: u16,
    pub allow: bool,
    pub body: Option<Value>,
}

impl McpLocalError {
    fn empty(status: u16) -> Self {
        Self {
            status,
            allow: status == 405,
            body: None,
        }
    }

    fn rpc(status: u16, id: Option<Value>, code: i32, message: &'static str, data: Option<Value>) -> Self {
        let mut error = Map::new();
        error.insert("code".to_string(), Value::Number(code.into()));
        error.insert("message".to_string(), Value::String(message.to_string()));
        if let Some(data) = data {
            error.insert("data".to_string(), data);
        }
        let mut body = Map::new();
        body.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        if let Some(id) = id {
            body.insert("id".to_string(), id);
        }
        body.insert("error".to_string(), Value::Object(error));
        Self {
            status,
            allow: false,
            body: Some(Value::Object(body)),
        }
    }

    fn header_mismatch(id: Option<Value>) -> Self {
        Self::rpc(400, id, -32020, "Header mismatch", None)
    }

    fn invalid_request(id: Option<Value>) -> Self {
        Self::rpc(400, id, -32600, "Invalid request", None)
    }

    fn invalid_params(id: Value) -> Self {
        Self::rpc(400, Some(id), -32602, "Invalid params", None)
    }
}

#[derive(Clone)]
pub(crate) struct McpRouteProof {
    pub backend: Arc<RuntimeMcpBackend>,
    pub backend_snapshot: EdgionBackendSnapshot,
    pub traffic_policy_snapshot: BackendTrafficPolicySnapshot,
    pub tls_policy_snapshot: BackendTLSPolicySnapshot,
}

pub(crate) enum McpRouteProofOutcome {
    NotMcp,
    Ready(McpRouteProof),
    Invalid,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct McpRouteCors {
    provider_count: usize,
    exact_origins: ExactCorsOriginSet,
    terminal_preflight_owner: bool,
}

pub(crate) enum McpRequestOutcome {
    ExistingCorsPreflight,
    Ready(McpRequestPlan),
    Reject(McpLocalError),
}

enum McpHeadOutcome {
    ExistingCorsPreflight,
    Post(ParsedHeaders),
    Reject(McpLocalError),
}

pub(crate) fn prove_mcp_route(ctx: &EdgionHttpContext) -> McpRouteProofOutcome {
    let Some(route) = ctx.route_unit.as_ref() else {
        return McpRouteProofOutcome::NotMcp;
    };
    let Ok(valid) = validate_mcp_http_route_rule(&route.rule) else {
        return McpRouteProofOutcome::NotMcp;
    };
    let Some(backend) =
        prove_compiled_mcp_backend(valid.backend_ref, &route.matched_info.rsc.ns, route.backends.as_slice())
    else {
        return McpRouteProofOutcome::Invalid;
    };
    let CompiledBackendTarget::EdgionBackend { namespace, name } = &backend.target else {
        return McpRouteProofOutcome::Invalid;
    };
    let snapshot = crate::ai::global_edgion_backend_store().edgion_backend_snapshot();
    let Some(runtime) = runtime_mcp_backend(snapshot.get_by_key(&format!("{namespace}/{name}"))) else {
        return McpRouteProofOutcome::NotMcp;
    };
    McpRouteProofOutcome::Ready(McpRouteProof {
        backend: runtime,
        backend_snapshot: snapshot,
        traffic_policy_snapshot: crate::backends::policy::backend_traffic::get_backend_traffic_policy_store()
            .snapshot(),
        tls_policy_snapshot: crate::backends::get_global_backend_tls_policy_store().snapshot(),
    })
}

fn runtime_mcp_backend(entry: Option<Arc<RuntimeEdgionBackend>>) -> Option<Arc<RuntimeMcpBackend>> {
    match entry.as_deref()? {
        RuntimeEdgionBackend::Mcp(runtime) => Some(Arc::clone(runtime)),
        RuntimeEdgionBackend::Ai(_) | RuntimeEdgionBackend::A2a(_) => None,
    }
}

fn prove_compiled_mcp_backend<'a>(
    raw: &HTTPBackendRef,
    route_namespace: &str,
    compiled: &'a [CompiledBackend],
) -> Option<&'a CompiledBackend> {
    let [backend] = compiled else {
        return None;
    };
    let expected_namespace = raw.namespace.as_deref().unwrap_or(route_namespace);
    let CompiledBackendTarget::EdgionBackend { namespace, name } = &backend.target else {
        return None;
    };
    if backend.slot != 0
        || backend.schema.group != raw.group
        || backend.schema.kind != raw.kind
        || backend.schema.namespace != raw.namespace
        || backend.schema.name != raw.name
        || backend.schema.port != raw.port
        || backend.schema.weight != raw.weight
        || backend.schema.ref_denied.is_some()
        || namespace != expected_namespace
        || name != &raw.name
    {
        return None;
    }
    Some(backend)
}

pub(crate) fn route_cors_evidence(ctx: &EdgionHttpContext) -> McpRouteCors {
    let mut out = McpRouteCors::default();
    if let Some(route) = &ctx.route_unit {
        if let Some(filters) = &route.rule.filters {
            for filter in filters {
                if filter.filter_type == HTTPRouteFilterType::CORS {
                    if let Some(cors) = &filter.cors {
                        out.provider_count = out.provider_count.saturating_add(1);
                        out.terminal_preflight_owner = true;
                        if let Some(origins) = &cors.allow_origins {
                            out.exact_origins
                                .extend_configured_literals(origins.iter().map(String::as_str));
                        }
                    }
                }
            }
        }
    }
    if let Some(runtimes) = &ctx.resolved_route_ext_refs {
        for runtime in runtimes.iter().flatten() {
            merge_cors_evidence(&mut out, runtime.mcp_cors_evidence());
        }
    }
    out
}

fn merge_cors_evidence(target: &mut McpRouteCors, evidence: &McpCorsEvidence) {
    target.provider_count = target.provider_count.saturating_add(evidence.provider_count);
    target.terminal_preflight_owner |= evidence.terminal_preflight_owner;
    target
        .exact_origins
        .extend_configured_literals(evidence.exact_origins.iter());
}

pub(crate) fn has_upstream_override(
    global: &[Arc<PluginRuntime>],
    gateway: &[Arc<PluginRuntime>],
    route: &PluginRuntime,
    extension_refs: &[Option<Arc<PluginRuntime>>],
) -> bool {
    route.may_override_upstream()
        || global.iter().any(|runtime| runtime.may_override_upstream())
        || gateway.iter().any(|runtime| runtime.may_override_upstream())
        || extension_refs
            .iter()
            .flatten()
            .any(|runtime| runtime.may_override_upstream())
}

pub(crate) fn validate_request(
    method: &Method,
    headers: &HeaderMap,
    body: &[u8],
    body_generation: u64,
    proof: McpRouteProof,
    cors: &McpRouteCors,
) -> McpRequestOutcome {
    let parsed_headers = match validate_request_head(method, headers, cors) {
        McpHeadOutcome::ExistingCorsPreflight => return McpRequestOutcome::ExistingCorsPreflight,
        McpHeadOutcome::Reject(error) => return McpRequestOutcome::Reject(error),
        McpHeadOutcome::Post(parsed) => parsed,
    };
    validate_request_body(body, Some(body_generation), proof, parsed_headers)
}

pub(crate) async fn capture_and_validate_request(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    proof: McpRouteProof,
    cors: &McpRouteCors,
) -> McpRequestOutcome {
    let parsed_headers = match validate_request_head(&session.req_header().method, &session.req_header().headers, cors)
    {
        McpHeadOutcome::ExistingCorsPreflight => return McpRequestOutcome::ExistingCorsPreflight,
        McpHeadOutcome::Reject(error) => return McpRequestOutcome::Reject(error),
        McpHeadOutcome::Post(parsed) => parsed,
    };
    if ctx.request_info.entry_raw_query.is_some() {
        return McpRequestOutcome::Reject(McpLocalError::empty(400));
    }
    let body = {
        let mut adapter = PingoraSessionAdapter::new(session, ctx);
        match adapter.read_request_body(Some(MCP_MAX_BODY_BYTES)).await {
            Ok(body) => body,
            Err(error) => return McpRequestOutcome::Reject(map_body_read_error(error)),
        }
    };
    validate_request_body(&body, ctx.request_body.generation(), proof, parsed_headers)
}

fn map_body_read_error(error: BodyReadError) -> McpLocalError {
    match error {
        BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
            McpLocalError::rpc(413, None, -32600, "Invalid request", None)
        }
        BodyReadError::Timeout => McpLocalError::rpc(408, None, -32600, "Invalid request", None),
        BodyReadError::Incomplete => McpLocalError::invalid_request(None),
        BodyReadError::Unimplemented
        | BodyReadError::NotBuffered
        | BodyReadError::GenerationConflict
        | BodyReadError::StorageFailure => McpLocalError::empty(500),
    }
}

fn validate_request_head(method: &Method, headers: &HeaderMap, cors: &McpRouteCors) -> McpHeadOutcome {
    let origins = headers.get_all("origin").iter().collect::<Vec<_>>();
    let origin_subtotal = origins.iter().fold(0_usize, |total, value| {
        total.saturating_add("origin".len() + value.as_bytes().len())
    });
    if origins.len() > 1 || origin_subtotal > MAX_RELEVANT_HEADER_BYTES {
        return McpHeadOutcome::Reject(McpLocalError::empty(403));
    }
    if cors.provider_count > 1 {
        return McpHeadOutcome::Reject(McpLocalError::empty(500));
    }
    if let Some(origin) = origins.first() {
        let Ok(origin) = origin.to_str() else {
            return McpHeadOutcome::Reject(McpLocalError::empty(403));
        };
        if cors.exact_origins.contains_request_origin(origin) != Ok(true) {
            return McpHeadOutcome::Reject(McpLocalError::empty(403));
        }
    }

    if method == Method::OPTIONS {
        let actual_preflight = headers.contains_key("origin") && headers.contains_key("access-control-request-method");
        return if actual_preflight && cors.terminal_preflight_owner {
            McpHeadOutcome::ExistingCorsPreflight
        } else {
            McpHeadOutcome::Reject(McpLocalError::empty(405))
        };
    }
    if method != Method::POST {
        return McpHeadOutcome::Reject(McpLocalError::empty(405));
    }

    let parsed_headers = match parse_headers(headers) {
        Ok(parsed) => parsed,
        Err(error) => return McpHeadOutcome::Reject(error),
    };
    if !valid_content_type(&parsed_headers.content_type) || !valid_accept(&parsed_headers.accept) {
        return McpHeadOutcome::Reject(McpLocalError::header_mismatch(None));
    }
    McpHeadOutcome::Post(parsed_headers)
}

fn validate_request_body(
    body: &[u8],
    body_generation: Option<u64>,
    proof: McpRouteProof,
    parsed_headers: ParsedHeaders,
) -> McpRequestOutcome {
    if body.len() as u64 > MCP_MAX_BODY_BYTES {
        return McpRequestOutcome::Reject(McpLocalError::rpc(413, None, -32600, "Invalid request", None));
    }

    let parsed = match parse_json_instance(body) {
        Ok(parsed) => parsed,
        Err(StrictJsonError::TooLarge) => {
            return McpRequestOutcome::Reject(McpLocalError::rpc(413, None, -32600, "Invalid request", None));
        }
        Err(StrictJsonError::DepthLimit | StrictJsonError::NodeLimit) => {
            return McpRequestOutcome::Reject(McpLocalError::invalid_request(None));
        }
        Err(StrictJsonError::Malformed | StrictJsonError::DuplicateKey) => {
            return McpRequestOutcome::Reject(McpLocalError::rpc(400, None, -32700, "Parse error", None));
        }
    };
    let Some(root) = parsed.value().as_object() else {
        return McpRequestOutcome::Reject(McpLocalError::invalid_request(None));
    };
    let id = match root.get("id") {
        Some(Value::String(value)) => Value::String(value.clone()),
        Some(Value::Number(value)) if valid_json_integer(value) => Value::Number(value.clone()),
        _ => return McpRequestOutcome::Reject(McpLocalError::invalid_request(None)),
    };
    if root.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return McpRequestOutcome::Reject(McpLocalError::invalid_request(Some(id)));
    }
    let Some(body_method) = root
        .get("method")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return McpRequestOutcome::Reject(McpLocalError::invalid_request(Some(id)));
    };
    let Some(params) = root.get("params").and_then(Value::as_object) else {
        return McpRequestOutcome::Reject(McpLocalError::invalid_request(Some(id)));
    };
    let Some(meta) = params.get("_meta").and_then(Value::as_object) else {
        return McpRequestOutcome::Reject(McpLocalError::invalid_request(Some(id)));
    };
    let Some(body_version) = meta
        .get("io.modelcontextprotocol/protocolVersion")
        .and_then(Value::as_str)
    else {
        return McpRequestOutcome::Reject(McpLocalError::invalid_request(Some(id)));
    };
    if parsed_headers.protocol_version != body_version {
        return McpRequestOutcome::Reject(McpLocalError::header_mismatch(Some(id)));
    }
    if body_version != MCP_PROTOCOL_VERSION {
        return McpRequestOutcome::Reject(McpLocalError::rpc(
            400,
            Some(id),
            -32022,
            "Unsupported protocol version",
            Some(json!({"requested": body_version, "supported": [MCP_PROTOCOL_VERSION]})),
        ));
    }
    if parsed_headers.method != body_method {
        return McpRequestOutcome::Reject(McpLocalError::header_mismatch(Some(id)));
    }
    let method = match body_method {
        "server/discover" => McpMethod::ServerDiscover,
        "tools/list" => McpMethod::ToolsList,
        "tools/call" => McpMethod::ToolsCall,
        _ => {
            return McpRequestOutcome::Reject(McpLocalError::rpc(404, Some(id), -32601, "Method not found", None));
        }
    };
    let Some(client_capabilities) = meta
        .get("io.modelcontextprotocol/clientCapabilities")
        .and_then(Value::as_object)
    else {
        return McpRequestOutcome::Reject(McpLocalError::invalid_request(Some(id)));
    };
    if !valid_client_capabilities(client_capabilities)
        || !valid_client_info(meta.get("io.modelcontextprotocol/clientInfo"))
    {
        return McpRequestOutcome::Reject(McpLocalError::invalid_request(Some(id)));
    }
    if params.contains_key("requestState") || params.contains_key("inputResponses") {
        return McpRequestOutcome::Reject(McpLocalError::invalid_params(id));
    }
    let name = if method == McpMethod::ToolsCall {
        let Some(body_name) = params.get("name").and_then(Value::as_str) else {
            return McpRequestOutcome::Reject(McpLocalError::invalid_params(id));
        };
        let Some(raw_header_name) = parsed_headers.name.as_ref() else {
            return McpRequestOutcome::Reject(McpLocalError::header_mismatch(Some(id)));
        };
        let header_name = match decode_mirror_value(raw_header_name.as_bytes()) {
            Ok(value) => value,
            Err(_) => return McpRequestOutcome::Reject(McpLocalError::header_mismatch(Some(id))),
        };
        if header_name != body_name {
            return McpRequestOutcome::Reject(McpLocalError::header_mismatch(Some(id)));
        }
        if params.get("arguments").is_some_and(|value| !value.is_object()) {
            return McpRequestOutcome::Reject(McpLocalError::invalid_params(id));
        }
        Some(body_name.to_string())
    } else {
        None
    };

    let Some(body_generation) = body_generation else {
        return McpRequestOutcome::Reject(McpLocalError::empty(500));
    };
    McpRequestOutcome::Ready(McpRequestPlan {
        method,
        correlation_id: id,
        name,
        name_header: parsed_headers.name,
        params: params.clone(),
        client_capabilities: client_capabilities.clone(),
        passthrough_param_headers: parsed_headers.param_headers,
        body_generation,
        backend: proof.backend,
        backend_snapshot: proof.backend_snapshot,
        traffic_policy_snapshot: proof.traffic_policy_snapshot,
        tls_policy_snapshot: proof.tls_policy_snapshot,
        selection: McpSelectionState::Validated,
    })
}

struct ParsedHeaders {
    content_type: String,
    accept: Vec<String>,
    protocol_version: String,
    method: String,
    name: Option<HeaderValue>,
    param_headers: Vec<McpParamHeader>,
}

fn parse_headers(headers: &HeaderMap) -> Result<ParsedHeaders, McpLocalError> {
    let mut occurrences = 0_usize;
    let mut bytes = 0_usize;
    let mut param_headers = Vec::new();
    let mut param_names = std::collections::HashSet::new();
    for (name, value) in headers.iter() {
        let name_text = name.as_str();
        if name_text == "origin"
            || name_text == "content-type"
            || name_text == "accept"
            || name_text.starts_with("mcp-")
        {
            occurrences = occurrences.saturating_add(1);
            bytes = bytes
                .saturating_add(name_text.len())
                .saturating_add(value.as_bytes().len());
        }
        if let Some(suffix) = name_text.strip_prefix("mcp-param-") {
            if suffix.is_empty() || !suffix.as_bytes().iter().copied().all(is_tchar) {
                return Err(McpLocalError::header_mismatch(None));
            }
            if !param_names.insert(suffix.to_ascii_lowercase()) || param_headers.len() >= MAX_PARAM_HEADER_OCCURRENCES {
                return Err(McpLocalError::header_mismatch(None));
            }
            decode_mirror_value(value.as_bytes())?;
            param_headers.push(McpParamHeader {
                name: name.clone(),
                value: value.clone(),
            });
        }
    }
    if occurrences > MAX_RELEVANT_HEADER_OCCURRENCES || bytes > MAX_RELEVANT_HEADER_BYTES {
        return Err(McpLocalError::header_mismatch(None));
    }
    let content_type = singleton_utf8(headers, "content-type", true)?;
    let protocol_version = singleton_utf8(headers, "mcp-protocol-version", true)?;
    let method = singleton_utf8(headers, "mcp-method", true)?;
    let name = singleton_bytes(headers, "mcp-name", false)?.cloned();
    if method != "tools/call"
        && name
            .as_ref()
            .is_some_and(|value| value.as_bytes().len() > MAX_DECODED_HEADER_VALUE_BYTES)
    {
        return Err(McpLocalError::header_mismatch(None));
    }
    let accept = headers
        .get_all("accept")
        .iter()
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .map_err(|_| McpLocalError::header_mismatch(None))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if accept.is_empty() {
        return Err(McpLocalError::header_mismatch(None));
    }
    Ok(ParsedHeaders {
        content_type,
        accept,
        protocol_version,
        method,
        name,
        param_headers,
    })
}

fn singleton_utf8(headers: &HeaderMap, name: &str, required: bool) -> Result<String, McpLocalError> {
    let value = singleton_bytes(headers, name, required)?;
    value
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .map_err(|_| McpLocalError::header_mismatch(None))
        })
        .transpose()?
        .ok_or_else(|| McpLocalError::header_mismatch(None))
}

fn singleton_bytes<'a>(
    headers: &'a HeaderMap,
    name: &str,
    required: bool,
) -> Result<Option<&'a HeaderValue>, McpLocalError> {
    let values = headers.get_all(name).iter().collect::<Vec<_>>();
    if values.len() > 1 || (required && values.is_empty()) {
        return Err(McpLocalError::header_mismatch(None));
    }
    Ok(values.first().copied())
}

fn decode_mirror_value(raw: &[u8]) -> Result<String, McpLocalError> {
    let decoded = if raw.starts_with(b"=?base64?") && raw.ends_with(b"?=") {
        let encoded = &raw[9..raw.len().saturating_sub(2)];
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| McpLocalError::header_mismatch(None))?;
        if base64::engine::general_purpose::STANDARD.encode(&decoded).as_bytes() != encoded {
            return Err(McpLocalError::header_mismatch(None));
        }
        decoded
    } else {
        if (raw.starts_with(b"=?") && raw.ends_with(b"?="))
            || raw.first().is_some_and(u8::is_ascii_whitespace)
            || raw.last().is_some_and(u8::is_ascii_whitespace)
            || !raw.iter().all(|byte| matches!(byte, 0x09 | 0x20..=0x7e))
        {
            return Err(McpLocalError::header_mismatch(None));
        }
        raw.to_vec()
    };
    if decoded.len() > MAX_DECODED_HEADER_VALUE_BYTES {
        return Err(McpLocalError::header_mismatch(None));
    }
    String::from_utf8(decoded).map_err(|_| McpLocalError::header_mismatch(None))
}

#[cfg(test)]
pub(crate) fn test_unselected_plan() -> McpRequestPlan {
    use crate::ai::{EdgionBackendStore, RuntimeExternalHttpEndpoint, RuntimeMcpTarget, RuntimeMcpTransport};

    let store = EdgionBackendStore::new();
    test_plan_for_proof(McpRouteProof {
        backend: Arc::new(RuntimeMcpBackend {
            uid: "test".to_string(),
            transport: RuntimeMcpTransport::StreamableHttp20260728,
            target: RuntimeMcpTarget::External(RuntimeExternalHttpEndpoint {
                domain: "example.com".to_string(),
                port: 443,
                use_tls: true,
                sni: "example.com".to_string(),
                authority: "example.com".to_string(),
                path_prefix: "/mcp".to_string(),
            }),
            bearer: None,
        }),
        backend_snapshot: store.edgion_backend_snapshot(),
        traffic_policy_snapshot: crate::backends::policy::backend_traffic::BackendTrafficPolicyStore::new().snapshot(),
        tls_policy_snapshot: crate::backends::BackendTLSPolicyStore::new().snapshot(),
    })
}

#[cfg(test)]
pub(crate) fn test_plan_for_proof(proof: McpRouteProof) -> McpRequestPlan {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert(
        "accept",
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    headers.insert("mcp-protocol-version", HeaderValue::from_static(MCP_PROTOCOL_VERSION));
    headers.insert("mcp-method", HeaderValue::from_static("tools/call"));
    headers.insert("mcp-name", HeaderValue::from_static("weather"));
    headers.append("mcp-param-region", HeaderValue::from_static("west"));
    headers.append("mcp-param-zone", HeaderValue::from_static("east"));
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"weather","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#;
    match validate_request(&Method::POST, &headers, body, 1, proof, &McpRouteCors::default()) {
        McpRequestOutcome::Ready(plan) => plan,
        _ => panic!("test MCP request must validate"),
    }
}

fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

fn valid_content_type(value: &str) -> bool {
    let mut parts = value.split(';').map(str::trim);
    if !parts
        .next()
        .is_some_and(|media| media.eq_ignore_ascii_case("application/json"))
    {
        return false;
    }
    match (parts.next(), parts.next()) {
        (None, None) => true,
        (Some(charset), None) => charset.split_once('=').is_some_and(|(name, value)| {
            let value = value.trim();
            let value = match value.strip_prefix('"').and_then(|value| value.strip_suffix('"')) {
                Some(value) => value,
                None if !value.contains('"') => value,
                None => return false,
            };
            name.trim().eq_ignore_ascii_case("charset") && value.eq_ignore_ascii_case("utf-8")
        }),
        _ => false,
    }
}

fn valid_accept(lines: &[String]) -> bool {
    let mut json_ok = false;
    let mut sse_ok = false;
    for member in lines.iter().flat_map(|line| line.split(',')).map(str::trim) {
        let mut parts = member.split(';').map(str::trim);
        let media = parts.next().unwrap_or_default();
        let mut allowed = true;
        let mut quality_seen = false;
        for parameter in parts {
            let Some((name, value)) = parameter.split_once('=') else {
                allowed = false;
                continue;
            };
            if name.trim().eq_ignore_ascii_case("q") {
                if quality_seen {
                    allowed = false;
                    continue;
                }
                quality_seen = true;
                allowed &= valid_positive_qvalue(value.trim());
            }
        }
        if allowed && media.eq_ignore_ascii_case("application/json") {
            json_ok = true;
        }
        if allowed && media.eq_ignore_ascii_case("text/event-stream") {
            sse_ok = true;
        }
    }
    json_ok && sse_ok
}

fn valid_positive_qvalue(value: &str) -> bool {
    match value.as_bytes() {
        b"1" => true,
        [b'1', b'.', tail @ ..] if tail.len() <= 3 => tail.iter().all(|byte| *byte == b'0'),
        [b'0', b'.', tail @ ..] if tail.len() <= 3 => {
            tail.iter().all(u8::is_ascii_digit) && tail.iter().any(|byte| *byte != b'0')
        }
        _ => false,
    }
}

fn valid_json_integer(value: &serde_json::Number) -> bool {
    value.is_i64()
        || value.is_u64()
        || value
            .as_f64()
            .is_some_and(|value| value.is_finite() && value.fract() == 0.0)
}

fn valid_json_schema_value(value: &Value) -> bool {
    match value {
        Value::Object(value) => value.values().all(valid_json_schema_value),
        Value::Array(value) => value.iter().all(valid_json_schema_value),
        Value::String(_) | Value::Bool(_) => true,
        Value::Number(value) => valid_json_integer(value),
        Value::Null => false,
    }
}

fn valid_json_object(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.values().all(valid_json_schema_value))
}

fn valid_client_capabilities(capabilities: &Map<String, Value>) -> bool {
    if let Some(elicitation) = capabilities.get("elicitation") {
        let Some(elicitation) = elicitation.as_object() else {
            return false;
        };
        for member in ["form", "url"] {
            if elicitation.get(member).is_some_and(|value| !valid_json_object(value)) {
                return false;
            }
        }
    }
    if let Some(experimental) = capabilities.get("experimental") {
        let Some(experimental) = experimental.as_object() else {
            return false;
        };
        if !experimental.values().all(valid_json_object) {
            return false;
        }
    }
    if let Some(extensions) = capabilities.get("extensions") {
        let Some(extensions) = extensions.as_object() else {
            return false;
        };
        if !extensions
            .iter()
            .all(|(key, value)| valid_extension_key(key) && valid_json_object(value))
        {
            return false;
        }
    }
    if capabilities.get("roots").is_some_and(|value| !value.is_object()) {
        return false;
    }
    if let Some(sampling) = capabilities.get("sampling") {
        let Some(sampling) = sampling.as_object() else {
            return false;
        };
        for member in ["context", "tools"] {
            if sampling.get(member).is_some_and(|value| !valid_json_object(value)) {
                return false;
            }
        }
    }
    true
}

fn valid_extension_key(key: &str) -> bool {
    let Some((prefix, name)) = key.split_once('/') else {
        return false;
    };
    if prefix.is_empty() || name.is_empty() || name.contains('/') {
        return false;
    }
    let valid_label = |label: &str| {
        let bytes = label.as_bytes();
        bytes.first().is_some_and(u8::is_ascii_alphabetic)
            && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
            && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    };
    let bytes = name.as_bytes();
    prefix.split('.').all(valid_label)
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_client_info(value: Option<&Value>) -> bool {
    let Some(value) = value else { return true };
    let Some(info) = value.as_object() else { return false };
    if !info.get("name").is_some_and(Value::is_string) || !info.get("version").is_some_and(Value::is_string) {
        return false;
    }
    for field in ["description", "title", "websiteUrl"] {
        if info.get(field).is_some_and(|value| !value.is_string()) {
            return false;
        }
    }
    info.get("icons").is_none_or(|icons| {
        icons.as_array().is_some_and(|icons| {
            icons.iter().all(|icon| {
                let Some(icon) = icon.as_object() else {
                    return false;
                };
                icon.get("src").is_some_and(Value::is_string)
                    && icon.get("mimeType").is_none_or(Value::is_string)
                    && icon
                        .get("sizes")
                        .is_none_or(|sizes| sizes.as_array().is_some_and(|sizes| sizes.iter().all(Value::is_string)))
                    && icon
                        .get("theme")
                        .is_none_or(|theme| matches!(theme.as_str(), Some("light" | "dark")))
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn headers(method: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        headers.append("accept", HeaderValue::from_static("application/json"));
        headers.append("accept", HeaderValue::from_static("text/event-stream"));
        headers.insert("mcp-protocol-version", HeaderValue::from_static(MCP_PROTOCOL_VERSION));
        headers.insert("mcp-method", HeaderValue::from_str(method).expect("method header"));
        headers
    }

    fn body(method: &str, extra: &str) -> Vec<u8> {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"{MCP_PROTOCOL_VERSION}","io.modelcontextprotocol/clientCapabilities":{{}}}}{extra}}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn strict_media_and_dual_accept_are_required() {
        assert!(valid_content_type("application/json"));
        assert!(valid_content_type("Application/JSON; Charset=\"UTF-8\""));
        assert!(!valid_content_type("application/json; profile=x"));
        assert!(valid_accept(&["application/json, text/event-stream".to_string()]));
        assert!(!valid_accept(&["*/*".to_string()]));
        assert!(!valid_accept(&["application/json, text/event-stream;q=0".to_string()]));
        assert!(!valid_accept(&[
            "application/json, text/event-stream;q=bogus".to_string()
        ]));
        assert!(!valid_accept(
            &["application/json, text/event-stream;q=1.1".to_string()]
        ));
        for quality in ["1e0", ".5", "01", "1.0000"] {
            assert!(!valid_accept(&[format!(
                "application/json, text/event-stream;q={quality}"
            )]));
        }
        assert!(!valid_content_type("application/json; charset=\"utf-8"));
    }

    #[test]
    fn sentinel_is_canonical_bounded_and_unambiguous() {
        assert_eq!(decode_mirror_value(b"plain").expect("plain"), "plain");
        assert_eq!(
            decode_mirror_value(b"=?base64?plain").expect("partial prefix is plain"),
            "=?base64?plain"
        );
        assert_eq!(
            decode_mirror_value(b"plain?=").expect("partial suffix is plain"),
            "plain?="
        );
        assert_eq!(decode_mirror_value(b"=?base64?w6k=?=").expect("encoded"), "é");
        assert!(decode_mirror_value(b" padded ").is_err());
        assert!(decode_mirror_value(b"=?BASE64?w6k=?=").is_err());
        assert!(decode_mirror_value(b"=?base64?bad!?=").is_err());
    }

    #[test]
    fn duplicate_singletons_and_param_names_fail() {
        let mut values = headers("server/discover");
        values.append("mcp-method", HeaderValue::from_static("server/discover"));
        assert!(parse_headers(&values).is_err());

        let mut values = headers("server/discover");
        values.append("mcp-param-region", HeaderValue::from_static("a"));
        values.append("mcp-param-region", HeaderValue::from_static("b"));
        assert!(parse_headers(&values).is_err());

        for name in ["content-type", "mcp-protocol-version", "mcp-method"] {
            let mut values = headers("server/discover");
            values.append(name, HeaderValue::from_static("duplicate"));
            assert!(parse_headers(&values).is_err(), "duplicate {name} must fail");
        }
        let mut values = headers("server/discover");
        values.append("mcp-name", HeaderValue::from_static("first"));
        values.append("mcp-name", HeaderValue::from_static("second"));
        assert!(parse_headers(&values).is_err(), "duplicate mcp-name must fail");

        let mut values = headers("server/discover");
        values.append("origin", HeaderValue::from_static("https://one.example"));
        values.append("origin", HeaderValue::from_static("https://two.example"));
        assert!(matches!(
            validate_request_head(&Method::POST, &values, &McpRouteCors::default()),
            McpHeadOutcome::Reject(McpLocalError { status: 403, .. })
        ));
    }

    #[test]
    fn inapplicable_name_is_still_singleton_and_bounded_without_decoding() {
        let mut values = headers("server/discover");
        values.insert(
            "mcp-name",
            HeaderValue::from_bytes(&vec![b'x'; MAX_DECODED_HEADER_VALUE_BYTES]).expect("exact name"),
        );
        assert!(parse_headers(&values).is_ok());
        values.insert(
            "mcp-name",
            HeaderValue::from_bytes(&vec![b'x'; MAX_DECODED_HEADER_VALUE_BYTES + 1]).expect("oversize name"),
        );
        assert!(parse_headers(&values).is_err());
    }

    #[test]
    fn repeated_and_combined_accept_lines_parse_equivalently() {
        let repeated = headers("server/discover");
        let mut combined = headers("server/discover");
        combined.remove("accept");
        combined.insert(
            "accept",
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        let repeated = parse_headers(&repeated).expect("repeated accept lines");
        let combined = parse_headers(&combined).expect("combined accept line");
        assert_eq!(valid_accept(&repeated.accept), valid_accept(&combined.accept));
        assert_eq!(repeated.content_type, combined.content_type);
        assert_eq!(repeated.protocol_version, combined.protocol_version);
        assert_eq!(repeated.method, combined.method);
    }

    #[test]
    fn header_occurrence_and_byte_budgets_accept_exact_and_reject_plus_one() {
        let mut occurrence_limited = headers("server/discover");
        for index in 0..59 {
            let name = HeaderName::from_bytes(format!("mcp-extra-{index}").as_bytes()).expect("header name");
            occurrence_limited.insert(name, HeaderValue::from_static("x"));
        }
        assert!(parse_headers(&occurrence_limited).is_ok());
        occurrence_limited.insert("mcp-extra-over", HeaderValue::from_static("x"));
        assert!(parse_headers(&occurrence_limited).is_err());

        let mut byte_limited = headers("server/discover");
        let base = byte_limited
            .iter()
            .filter(|(name, _)| {
                matches!(name.as_str(), "origin" | "content-type" | "accept") || name.as_str().starts_with("mcp-")
            })
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum::<usize>();
        let padding_name = "mcp-padding";
        let exact_len = MAX_RELEVANT_HEADER_BYTES - base - padding_name.len();
        byte_limited.insert(
            padding_name,
            HeaderValue::from_bytes(&vec![b'x'; exact_len]).expect("exact padding"),
        );
        assert!(parse_headers(&byte_limited).is_ok());
        byte_limited.insert(
            padding_name,
            HeaderValue::from_bytes(&vec![b'x'; exact_len + 1]).expect("plus-one padding"),
        );
        assert!(parse_headers(&byte_limited).is_err());
    }

    #[test]
    fn param_header_count_accepts_exact_and_rejects_plus_one() {
        let mut values = headers("server/discover");
        for index in 0..MAX_PARAM_HEADER_OCCURRENCES {
            let name = HeaderName::from_bytes(format!("mcp-param-p{index}").as_bytes()).expect("parameter name");
            values.insert(name, HeaderValue::from_static("x"));
        }
        assert!(parse_headers(&values).is_ok());
        values.insert("mcp-param-over", HeaderValue::from_static("x"));
        assert!(parse_headers(&values).is_err());
    }

    #[test]
    fn shared_json_parser_keeps_integer_id_and_rejects_duplicate_batch_and_limits() {
        let parsed = parse_json_instance(&body("server/discover", "")).expect("request");
        assert_eq!(parsed.value()["id"], json!(1));
        assert_eq!(
            parse_json_instance(br#"{"a":1,"a":2}"#).expect_err("duplicate key"),
            StrictJsonError::DuplicateKey
        );
        assert!(parse_json_instance(br#"[]"#).is_ok());

        let exact_depth = format!("{}0{}", "[".repeat(63), "]".repeat(63));
        assert!(parse_json_instance(exact_depth.as_bytes()).is_ok());
        let deep = format!("{}0{}", "[".repeat(64), "]".repeat(64));
        assert_eq!(
            parse_json_instance(deep.as_bytes()).expect_err("depth limit"),
            StrictJsonError::DepthLimit
        );

        let exact_nodes = serde_json::to_vec(&Value::Array(vec![Value::Null; 19_999])).expect("exact nodes");
        assert!(parse_json_instance(&exact_nodes).is_ok());
        let excess_nodes = serde_json::to_vec(&Value::Array(vec![Value::Null; 20_000])).expect("excess nodes");
        assert_eq!(
            parse_json_instance(&excess_nodes).expect_err("node limit"),
            StrictJsonError::NodeLimit
        );
    }

    #[test]
    fn origin_provider_and_options_policy_are_fail_closed() {
        let proof = test_proof();
        let mut cors = McpRouteCors {
            provider_count: 1,
            terminal_preflight_owner: true,
            ..McpRouteCors::default()
        };
        cors.exact_origins.insert_configured_literal("https://app.example");
        let mut request_headers = headers("server/discover");
        request_headers.insert("origin", HeaderValue::from_static("https://app.example"));
        request_headers.insert("access-control-request-method", HeaderValue::from_static("POST"));
        assert!(matches!(
            validate_request(&Method::OPTIONS, &request_headers, &[], 0, proof.clone(), &cors),
            McpRequestOutcome::ExistingCorsPreflight
        ));
        request_headers.remove("origin");
        assert!(matches!(
            validate_request(&Method::OPTIONS, &request_headers, &[], 0, proof.clone(), &cors),
            McpRequestOutcome::Reject(McpLocalError { status: 405, .. })
        ));
        cors.provider_count = 2;
        assert!(matches!(
            validate_request(
                &Method::POST,
                &headers("server/discover"),
                &body("server/discover", ""),
                0,
                proof,
                &cors
            ),
            McpRequestOutcome::Reject(McpLocalError { status: 500, .. })
        ));

        let mut absent_provider = headers("server/discover");
        absent_provider.insert("origin", HeaderValue::from_static("https://app.example"));
        assert!(matches!(
            validate_request(
                &Method::POST,
                &absent_provider,
                &body("server/discover", ""),
                0,
                test_proof(),
                &McpRouteCors::default()
            ),
            McpRequestOutcome::Reject(McpLocalError { status: 403, .. })
        ));
    }

    #[test]
    fn dangling_extension_ref_contributes_no_cors_provider() {
        let mut ctx = EdgionHttpContext::new();
        ctx.resolved_route_ext_refs = Some(Arc::new(vec![None]));
        let evidence = route_cors_evidence(&ctx);
        assert_eq!(evidence.provider_count, 0);
        assert!(evidence.exact_origins.is_empty());
        assert!(!evidence.terminal_preflight_owner);
    }

    #[test]
    fn route_proof_requires_exactly_the_validated_compiled_backend() {
        let rule: edgion_resources::resources::HTTPRouteRule = serde_json::from_value(json!({
            "matches": [{"path": {"type": "Exact", "value": "/mcp"}}],
            "backendRefs": [{
                "group": "edgion.io",
                "kind": "EdgionBackend",
                "name": "backend"
            }]
        }))
        .expect("MCP route rule");
        let valid = validate_mcp_http_route_rule(&rule).expect("validated rule");
        let compiled =
            CompiledBackend::compile(valid.backend_ref.clone(), "default", 0, Arc::new(PluginRuntime::new()));
        assert!(prove_compiled_mcp_backend(valid.backend_ref, "default", std::slice::from_ref(&compiled)).is_some());
        assert!(prove_compiled_mcp_backend(valid.backend_ref, "default", &[]).is_none());
        assert!(
            prove_compiled_mcp_backend(valid.backend_ref, "default", &[compiled.clone(), compiled.clone()]).is_none()
        );

        let mut wrong_slot = compiled.clone();
        wrong_slot.slot = 1;
        assert!(prove_compiled_mcp_backend(valid.backend_ref, "default", &[wrong_slot]).is_none());
        let mut wrong_name = compiled.clone();
        wrong_name.schema.name = "other".to_string();
        assert!(prove_compiled_mcp_backend(valid.backend_ref, "default", &[wrong_name]).is_none());
        let mut denied = compiled;
        denied.schema.ref_denied = Some(edgion_resources::resources::common::RefDenied {
            target_namespace: "default".to_string(),
            target_name: "backend".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        });
        assert!(prove_compiled_mcp_backend(valid.backend_ref, "default", &[denied]).is_none());
    }

    #[test]
    fn route_without_a_matched_rule_is_not_mcp() {
        assert!(matches!(
            prove_mcp_route(&EdgionHttpContext::new()),
            McpRouteProofOutcome::NotMcp
        ));
    }

    #[test]
    fn runtime_backend_classifier_accepts_only_mcp() {
        use std::collections::{BTreeSet, HashMap, HashSet};

        use edgion_resources::resources::edgion_backend::AiProvider;
        use edgion_resources::resources::edgion_plugins::OnRedisFailure;

        use crate::ai::{
            RuntimeA2aBackend, RuntimeA2aTarget, RuntimeAiBackend, RuntimeAiCredentialPool, RuntimeAiEndpoint,
        };
        use crate::plugins::common::redis_rate_limiter::RedisRateLimiter;

        assert!(runtime_mcp_backend(None).is_none());

        let a2a = RuntimeEdgionBackend::A2a(Arc::new(RuntimeA2aBackend {
            uid: "a2a".to_string(),
            target: RuntimeA2aTarget::External(crate::ai::RuntimeExternalHttpEndpoint {
                domain: "agent.example".to_string(),
                port: 443,
                use_tls: true,
                sni: "agent.example".to_string(),
                authority: "agent.example".to_string(),
                path_prefix: "/a2a".to_string(),
            }),
        }));
        assert!(runtime_mcp_backend(Some(Arc::new(a2a))).is_none());

        let ai = RuntimeEdgionBackend::Ai(Arc::new(RuntimeAiBackend {
            uid: "ai".to_string(),
            provider: AiProvider::OpenAICompatible,
            endpoint: RuntimeAiEndpoint {
                domain: "ai.example".to_string(),
                port: 443,
                use_tls: true,
                sni: "ai.example".to_string(),
                host: "ai.example".to_string(),
                path_prefix: String::new(),
            },
            streaming: false,
            max_output_tokens: None,
            credential_pool: RuntimeAiCredentialPool {
                redis_ref: None,
                on_redis_failure: OnRedisFailure::Deny,
                credentials: Vec::new(),
                limiter_core: Arc::new(RedisRateLimiter::new()),
            },
            models: HashMap::new(),
            wildcard_models: Vec::new(),
            canonical_models: HashSet::new(),
            public_model_ids: BTreeSet::new(),
        }));
        assert!(runtime_mcp_backend(Some(Arc::new(ai))).is_none());

        let expected = test_proof().backend;
        let classified = runtime_mcp_backend(Some(Arc::new(RuntimeEdgionBackend::Mcp(Arc::clone(&expected)))))
            .expect("MCP runtime must classify");
        assert!(Arc::ptr_eq(&expected, &classified));
    }

    #[test]
    fn unsupported_method_precedes_inapplicable_bad_name() {
        let mut request_headers = headers("ping");
        request_headers.insert("mcp-name", HeaderValue::from_static("=?base64?bad!?="));
        let outcome = validate_request(
            &Method::POST,
            &request_headers,
            &body("ping", ""),
            7,
            test_proof(),
            &McpRouteCors::default(),
        );
        let McpRequestOutcome::Reject(error) = outcome else {
            panic!("expected rejection")
        };
        assert_eq!(error.status, 404);
        assert_eq!(error.body.expect("body")["error"]["code"], -32601);
    }

    #[test]
    fn json_rpc_error_precedence_and_fixed_codes_are_pinned() {
        fn reject_code(headers: &HeaderMap, body: &[u8]) -> (u16, i64) {
            let McpRequestOutcome::Reject(error) =
                validate_request(&Method::POST, headers, body, 1, test_proof(), &McpRouteCors::default())
            else {
                panic!("expected rejection")
            };
            let code = error
                .body
                .as_ref()
                .and_then(|body| body["error"]["code"].as_i64())
                .expect("JSON-RPC code");
            (error.status, code)
        }

        assert_eq!(reject_code(&headers("server/discover"), b"{"), (400, -32700));
        assert_eq!(
            reject_code(&headers("server/discover"), br#"{"a":1,"a":2}"#),
            (400, -32700)
        );
        for invalid in [json!([]), json!(true), json!({"jsonrpc": "2.0", "id": 1, "result": {}})] {
            assert_eq!(
                reject_code(
                    &headers("server/discover"),
                    &serde_json::to_vec(&invalid).expect("serialize")
                ),
                (400, -32600)
            );
        }

        let mut notification: Value = serde_json::from_slice(&body("server/discover", "")).expect("body");
        notification.as_object_mut().expect("object").remove("id");
        assert_eq!(
            reject_code(
                &headers("server/discover"),
                &serde_json::to_vec(&notification).expect("serialize")
            ),
            (400, -32600)
        );

        let mut mismatch = headers("server/discover");
        mismatch.insert("mcp-method", HeaderValue::from_static("tools/list"));
        assert_eq!(reject_code(&mismatch, &body("server/discover", "")), (400, -32020));

        let unsupported = "2026-08-01";
        let mut unsupported_headers = headers("server/discover");
        unsupported_headers.insert("mcp-protocol-version", HeaderValue::from_static(unsupported));
        let unsupported_body = body("server/discover", "")
            .windows(MCP_PROTOCOL_VERSION.len())
            .position(|window| window == MCP_PROTOCOL_VERSION.as_bytes())
            .map(|offset| {
                let mut value = body("server/discover", "");
                value.splice(offset..offset + MCP_PROTOCOL_VERSION.len(), unsupported.bytes());
                value
            })
            .expect("version occurrence");
        assert_eq!(reject_code(&unsupported_headers, &unsupported_body), (400, -32022));

        assert_eq!(
            reject_code(
                &headers("server/discover"),
                &body("server/discover", r#", "requestState":"opaque""#)
            ),
            (400, -32602)
        );

        let invalid_capabilities = json!(true);
        let with_invalid_capabilities = |method: &str, version: &str| {
            let mut value: Value = serde_json::from_slice(&body(method, "")).expect("body");
            value["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!(version);
            value["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"] = invalid_capabilities.clone();
            serde_json::to_vec(&value).expect("serialize")
        };

        assert_eq!(
            reject_code(
                &headers("server/discover"),
                &with_invalid_capabilities("server/discover", "2026-08-01")
            ),
            (400, -32020)
        );
        let mut unsupported_headers = headers("server/discover");
        unsupported_headers.insert("mcp-protocol-version", HeaderValue::from_static("2026-08-01"));
        assert_eq!(
            reject_code(
                &unsupported_headers,
                &with_invalid_capabilities("server/discover", "2026-08-01")
            ),
            (400, -32022)
        );
        assert_eq!(
            reject_code(
                &headers("tools/list"),
                &with_invalid_capabilities("server/discover", MCP_PROTOCOL_VERSION)
            ),
            (400, -32020)
        );
        assert_eq!(
            reject_code(
                &headers("ping"),
                &with_invalid_capabilities("ping", MCP_PROTOCOL_VERSION)
            ),
            (404, -32601)
        );
    }

    #[test]
    fn tools_call_validates_name_and_captures_generation() {
        let mut request_headers = headers("tools/call");
        request_headers.insert("mcp-name", HeaderValue::from_static("weather"));
        request_headers.insert("mcp-param-region", HeaderValue::from_static("us-west1"));
        let outcome = validate_request(
            &Method::POST,
            &request_headers,
            &body("tools/call", r#","name":"weather","arguments":{}"#),
            9,
            test_proof(),
            &McpRouteCors::default(),
        );
        let McpRequestOutcome::Ready(plan) = outcome else {
            panic!("expected plan")
        };
        assert_eq!(plan.method, McpMethod::ToolsCall);
        assert_eq!(plan.correlation_id, json!(1));
        assert_eq!(plan.body_generation, 9);
        assert_eq!(plan.passthrough_param_headers.len(), 1);

        assert!(matches!(
            validate_request(
                &Method::POST,
                &request_headers,
                &body("tools/call", r#", "name":"weather""#),
                10,
                test_proof(),
                &McpRouteCors::default()
            ),
            McpRequestOutcome::Ready(_)
        ));
        assert!(matches!(
            validate_request(
                &Method::POST,
                &request_headers,
                &body("tools/call", r#", "name":"weather", "arguments":[]"#),
                11,
                test_proof(),
                &McpRouteCors::default()
            ),
            McpRequestOutcome::Reject(McpLocalError { status: 400, .. })
        ));
    }

    #[test]
    fn request_id_matches_pinned_string_or_integer_schema() {
        for id in [json!(1), json!(-2), json!(1.0), json!("request-1")] {
            let mut value: Value = serde_json::from_slice(&body("server/discover", "")).expect("body");
            value["id"] = id;
            assert!(matches!(
                validate_request(
                    &Method::POST,
                    &headers("server/discover"),
                    &serde_json::to_vec(&value).expect("serialize"),
                    1,
                    test_proof(),
                    &McpRouteCors::default()
                ),
                McpRequestOutcome::Ready(_)
            ));
        }
        for id in [
            json!(1.5),
            json!(1.5e-1),
            Value::Null,
            json!(true),
            json!({}),
            json!([]),
        ] {
            let mut value: Value = serde_json::from_slice(&body("server/discover", "")).expect("body");
            value["id"] = id;
            assert!(matches!(
                validate_request(
                    &Method::POST,
                    &headers("server/discover"),
                    &serde_json::to_vec(&value).expect("serialize"),
                    1,
                    test_proof(),
                    &McpRouteCors::default()
                ),
                McpRequestOutcome::Reject(McpLocalError { status: 400, .. })
            ));
        }
    }

    #[test]
    fn pinned_client_capability_and_implementation_shapes_are_enforced() {
        let valid = json!({
            "elicitation": {"form": {}},
            "experimental": {"feature": {}},
            "extensions": {"com.example/feature": {}},
            "roots": {},
            "sampling": {"context": {}, "tools": {}}
        });
        assert!(valid_client_capabilities(valid.as_object().expect("capabilities")));
        for invalid in [
            json!({"roots": true}),
            json!({"sampling": []}),
            json!({"elicitation": {"form": false}}),
            json!({"experimental": {"feature": false}}),
            json!({"extensions": {"missing-prefix": {}}}),
            json!({"extensions": {"com.-bad/feature": {}}}),
            json!({"extensions": {"com.example/feature": null}}),
        ] {
            assert!(!valid_client_capabilities(invalid.as_object().expect("capabilities")));
        }

        assert!(valid_client_info(Some(&json!({
            "name": "client",
            "version": "1",
            "icons": [{"src": "https://example.com/icon.png", "sizes": ["48x48"], "theme": "dark"}]
        }))));
        for invalid in [
            json!({"name": "client", "version": "1", "icons": [{}]}),
            json!({"name": "client", "version": "1", "icons": [{"src": "x", "sizes": [1]}]}),
            json!({"name": "client", "version": "1", "icons": [{"src": "x", "theme": "auto"}]}),
        ] {
            assert!(!valid_client_info(Some(&invalid)));
        }
    }

    #[test]
    fn body_read_errors_keep_frozen_status_and_body_policy() {
        let cases = [
            (BodyReadError::BufferLimitExceeded { limit: 1 }, 413, true),
            (BodyReadError::TooLarge { limit: 1 }, 413, true),
            (BodyReadError::Timeout, 408, true),
            (BodyReadError::Incomplete, 400, true),
            (BodyReadError::Unimplemented, 500, false),
            (BodyReadError::NotBuffered, 500, false),
            (BodyReadError::GenerationConflict, 500, false),
            (BodyReadError::StorageFailure, 500, false),
        ];
        for (error, status, has_rpc_body) in cases {
            let mapped = map_body_read_error(error);
            assert_eq!(mapped.status, status);
            assert_eq!(mapped.body.is_some(), has_rpc_body);
            if let Some(body) = mapped.body {
                assert_eq!(body["error"]["code"], -32600);
                assert!(body.get("id").is_none());
            }
        }
    }

    #[test]
    fn logical_body_limit_accepts_exact_and_rejects_plus_one() {
        let mut exact = body("server/discover", "");
        exact.resize(MCP_MAX_BODY_BYTES as usize, b' ');
        assert!(matches!(
            validate_request(
                &Method::POST,
                &headers("server/discover"),
                &exact,
                1,
                test_proof(),
                &McpRouteCors::default()
            ),
            McpRequestOutcome::Ready(_)
        ));
        exact.push(b' ');
        assert!(matches!(
            validate_request(
                &Method::POST,
                &headers("server/discover"),
                &exact,
                1,
                test_proof(),
                &McpRouteCors::default()
            ),
            McpRequestOutcome::Reject(McpLocalError { status: 413, .. })
        ));
    }

    #[tokio::test]
    async fn live_h1_capture_uses_shared_body_storage_and_binds_generation() {
        let request_body = body("server/discover", "");
        let raw = format!(
            "POST /mcp HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {MCP_PROTOCOL_VERSION}\r\nMcp-Method: server/discover\r\nContent-Length: {}\r\n\r\n",
            request_body.len()
        );
        let (mut client, server) = tokio::io::duplex(raw.len() + request_body.len() + 64);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client.write_all(raw.as_bytes()).await.expect("request headers");
        client.write_all(&request_body).await.expect("request body");
        drop(client);
        assert!(session.read_request().await.expect("read request"));

        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        let outcome =
            capture_and_validate_request(&mut session, &mut ctx, test_proof(), &McpRouteCors::default()).await;
        let McpRequestOutcome::Ready(plan) = outcome else {
            panic!("expected captured request plan")
        };
        assert!(ctx.request_body.has_cached_body());
        assert!(plan.body_generation_matches(&ctx));
        ctx.overwrite_request_body(bytes::Bytes::from_static(b"{}"))
            .expect("mutate cached body");
        assert!(!plan.body_generation_matches(&ctx));
    }

    #[tokio::test]
    async fn entry_query_is_rejected_before_body_capture_even_if_live_uri_was_rewritten() {
        let request_body = body("server/discover", "");
        let raw = format!(
            "POST /mcp HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {MCP_PROTOCOL_VERSION}\r\nMcp-Method: server/discover\r\nContent-Length: {}\r\n\r\n",
            request_body.len()
        );
        let (mut client, server) = tokio::io::duplex(raw.len() + request_body.len() + 64);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client.write_all(raw.as_bytes()).await.expect("request headers");
        client.write_all(&request_body).await.expect("request body");
        drop(client);
        assert!(session.read_request().await.expect("read request"));

        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.entry_raw_query = Some("secret=value".to_string());
        ctx.request_info.body_declared = Some(true);
        ctx.request_info.in_request_body_stage = true;
        let outcome =
            capture_and_validate_request(&mut session, &mut ctx, test_proof(), &McpRouteCors::default()).await;

        assert!(matches!(
            outcome,
            McpRequestOutcome::Reject(McpLocalError { status: 400, .. })
        ));
        assert!(!ctx.request_body.has_cached_body());
    }

    fn test_proof() -> McpRouteProof {
        use crate::ai::EdgionBackendStore;
        use crate::ai::{RuntimeExternalHttpEndpoint, RuntimeMcpTarget, RuntimeMcpTransport};

        let store = EdgionBackendStore::new();
        McpRouteProof {
            backend: Arc::new(RuntimeMcpBackend {
                uid: "test".to_string(),
                transport: RuntimeMcpTransport::StreamableHttp20260728,
                target: RuntimeMcpTarget::External(RuntimeExternalHttpEndpoint {
                    domain: "example.com".to_string(),
                    port: 443,
                    use_tls: true,
                    sni: "example.com".to_string(),
                    authority: "example.com".to_string(),
                    path_prefix: "/mcp".to_string(),
                }),
                bearer: None,
            }),
            backend_snapshot: store.edgion_backend_snapshot(),
            traffic_policy_snapshot: crate::backends::policy::backend_traffic::BackendTrafficPolicyStore::new()
                .snapshot(),
            tls_policy_snapshot: crate::backends::BackendTLSPolicyStore::new().snapshot(),
        }
    }

    #[test]
    fn installed_validated_plan_is_the_monotonic_main_proxy_no_replay_latch() {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.main_proxy_replay_allowed());
        assert!(!ctx.mcp_log_redaction_required());
        ctx.mcp_route_recognized = true;
        assert!(ctx.mcp_log_redaction_required());
        assert!(
            ctx.main_proxy_replay_allowed(),
            "recognition alone does not create an upstream replay state"
        );
        ctx.mcp_route_recognized = false;
        let mut ordinary = pingora_core::RetryType::ReusedOnly;
        assert!(!ctx.force_mcp_no_replay(&mut ordinary));
        assert_eq!(ordinary, pingora_core::RetryType::ReusedOnly);

        ctx.request_body_stream_active = true;
        assert!(!ctx.main_proxy_replay_allowed());
        ctx.request_body_stream_active = false;

        ctx.mcp_request_plan = Some(test_unselected_plan());
        assert!(ctx.mcp_no_replay());
        assert!(!ctx.main_proxy_replay_allowed());
        let mut reused = pingora_core::RetryType::ReusedOnly;
        assert!(ctx.force_mcp_no_replay(&mut reused));
        assert_eq!(reused, pingora_core::RetryType::Decided(false));
        let mut predecided = pingora_core::RetryType::Decided(true);
        assert!(ctx.force_mcp_no_replay(&mut predecided));
        assert_eq!(predecided, pingora_core::RetryType::Decided(false));
        ctx.request_body_stream_active = true;
        assert!(!ctx.main_proxy_replay_allowed());
    }
}
