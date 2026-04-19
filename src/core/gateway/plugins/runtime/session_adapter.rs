use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Uri};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::collections::HashMap;

use super::log::{EdgionPluginsLog, EdgionPluginsLogToken, PluginLog};
use super::traits::{PluginSession, PluginSessionError, PluginSessionResult};
use crate::core::gateway::runtime::server::http_to_grpc_status;
use crate::types::common::{KeyGet, KeySet, WebhookExtract};
use crate::types::filters::PluginRunningResult;
use crate::types::{DirectEndpointPreset, EdgionHttpContext, ExternalJumpPreset, InternalJumpPreset, MirrorState};

pub struct PingoraSessionAdapter<'a> {
    inner: &'a mut Session,
    ctx: &'a mut EdgionHttpContext,
    response_header: Option<&'a mut ResponseHeader>,
    /// Set to true after a gRPC trailers-only error response has been sent.
    /// Subsequent write_response_body and shutdown calls become no-ops.
    grpc_response_terminated: bool,
}

impl<'a> PingoraSessionAdapter<'a> {
    #[inline]
    pub fn new(inner: &'a mut Session, ctx: &'a mut EdgionHttpContext) -> Self {
        Self {
            inner,
            ctx,
            response_header: None,
            grpc_response_terminated: false,
        }
    }

    #[inline]
    pub fn with_response_header(
        inner: &'a mut Session,
        ctx: &'a mut EdgionHttpContext,
        response_header: &'a mut ResponseHeader,
    ) -> Self {
        Self {
            inner,
            ctx,
            response_header: Some(response_header),
            grpc_response_terminated: false,
        }
    }

    #[inline]
    pub fn set_terminate(&mut self) {
        self.ctx.plugin_running_result = PluginRunningResult::ErrTerminateRequest;
    }

    /// Send a gRPC error response with user-specified grpc-status and grpc-message.
    ///
    /// Uses the same three-frame pattern as `send_grpc_error_three_frame`:
    /// 1. HEADERS (HTTP 200 + content-type: application/grpc)
    /// 2. DATA (empty)
    /// 3. TRAILERS (grpc-status + grpc-message)
    pub async fn send_grpc_error_direct(&mut self, grpc_status: u16, message: Option<&str>) -> PluginSessionResult<()> {
        self.ctx.request_info.status = Some(200);
        self.ctx.request_info.grpc_status = Some(grpc_status);
        self.ctx.request_info.response_committed = true;
        self.grpc_response_terminated = true;

        let mut resp = ResponseHeader::build(200, None).map_err(|e| Box::new(e) as PluginSessionError)?;
        resp.insert_header("content-type", "application/grpc")
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner
            .write_response_header(Box::new(resp), false)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner
            .write_response_body(Some(Bytes::new()), false)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)?;

        let mut trailers = HeaderMap::new();
        trailers.insert(
            http::header::HeaderName::from_static("grpc-status"),
            HeaderValue::from_str(&grpc_status.to_string()).unwrap(),
        );
        if let Some(msg) = message {
            // Per gRPC spec, grpc-message must be percent-encoded for non-ASCII chars
            let encoded: String = msg
                .chars()
                .map(|c| {
                    if c.is_ascii_graphic() || c == ' ' {
                        c.to_string()
                    } else {
                        let mut buf = [0u8; 4];
                        c.encode_utf8(&mut buf)
                            .as_bytes()
                            .iter()
                            .map(|b| format!("%{:02X}", b))
                            .collect()
                    }
                })
                .collect();
            if let Ok(val) = HeaderValue::from_str(&encoded) {
                trailers.insert(http::header::HeaderName::from_static("grpc-message"), val);
            }
        }
        self.inner
            .as_downstream_mut()
            .write_response_trailers(trailers)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)
    }

    /// Resolve a key value by calling an external webhook service.
    ///
    /// Looks up the WebhookRuntime from the global registry, performs health/rate-limit
    /// checks, sends the HTTP request with retry, and extracts the value from the response.
    ///
    /// TODO: Full implementation in Phase 3 (WebhookRegistry + runtime)
    async fn resolve_webhook(&self, webhook_ref: &str, extract: &WebhookExtract) -> Option<String> {
        use crate::core::gateway::link_sys::providers::webhook::resolve_webhook_key;
        resolve_webhook_key(self, webhook_ref, extract).await
    }

    /// Extract path parameters from route pattern (lazy extraction).
    ///
    /// Parses the route pattern (e.g., "/api/:uid/profile") against the actual
    /// request path (e.g., "/api/123/profile") and extracts named parameters.
    fn extract_path_params(&mut self) {
        let mut params = HashMap::new();

        // Get the route pattern from matched route
        let pattern = self
            .ctx
            .route_unit
            .as_ref()
            .and_then(|ru| ru.matched_info.m.path.as_ref())
            .and_then(|p| p.value.as_ref())
            .map(|s| s.as_str());

        if let Some(pattern) = pattern {
            // Only parse if pattern contains parameters
            if pattern.contains(':') {
                let actual_path = self.inner.req_header().uri.path();
                params = Self::parse_path_params(pattern, actual_path);
            }
        }

        self.ctx.path_params = Some(params);
    }

    /// Send a gRPC trailers-only error response using the three-frame pattern:
    /// HEADERS (no END_STREAM) -> empty DATA -> TRAILERS (grpc-status + grpc-message).
    ///
    /// Pingora's proxy Session.write_response_header() does NOT propagate end_of_stream
    /// to the underlying H2 session, so a single HEADERS+END_STREAM frame cannot be used.
    async fn send_grpc_error_three_frame(
        inner: &mut Session,
        ctx: &mut EdgionHttpContext,
        http_status: u16,
    ) -> PluginSessionResult<()> {
        let (grpc_status, grpc_message) = http_to_grpc_status(http_status);
        ctx.request_info.status = Some(200);
        ctx.request_info.grpc_status = Some(grpc_status);
        ctx.request_info.response_committed = true;

        let mut resp = ResponseHeader::build(200, None).map_err(|e| Box::new(e) as PluginSessionError)?;
        resp.insert_header("content-type", "application/grpc")
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        inner
            .write_response_header(Box::new(resp), false)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        inner
            .write_response_body(Some(Bytes::new()), false)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)?;

        let mut trailers = HeaderMap::new();
        trailers.insert(
            http::header::HeaderName::from_static("grpc-status"),
            HeaderValue::from_str(&grpc_status.to_string()).unwrap(),
        );
        trailers.insert(
            http::header::HeaderName::from_static("grpc-message"),
            HeaderValue::from_static(grpc_message),
        );
        inner
            .as_downstream_mut()
            .write_response_trailers(trailers)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)
    }

    /// Parse path parameters from pattern and actual path.
    ///
    /// # Arguments
    /// * `pattern` - Route pattern with `:param` syntax (e.g., "/api/:uid/profile")
    /// * `actual_path` - Actual request path (e.g., "/api/123/profile")
    ///
    /// # Returns
    /// HashMap of parameter names to values.
    fn parse_path_params(pattern: &str, actual_path: &str) -> HashMap<String, String> {
        let mut params = HashMap::new();

        let pattern_segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
        let actual_segments: Vec<&str> = actual_path.split('/').filter(|s| !s.is_empty()).collect();

        for (i, pat_seg) in pattern_segments.iter().enumerate() {
            // Check if this is a parameter segment (starts with ':' but not '::')
            if pat_seg.starts_with(':') && !pat_seg.starts_with("::") {
                // Extract parameter name (skip the ':')
                let param_name = &pat_seg[1..];
                if !param_name.is_empty() {
                    if let Some(&actual_value) = actual_segments.get(i) {
                        params.insert(param_name.to_string(), actual_value.to_string());
                    }
                }
            }
        }

        params
    }
}

#[async_trait]
impl<'a> PluginSession for PingoraSessionAdapter<'a> {
    fn header_value(&self, name: &str) -> Option<String> {
        self.inner
            .req_header()
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|s| s.to_string())
    }

    fn request_headers(&self) -> Vec<(String, String)> {
        self.inner
            .req_header()
            .headers
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str().to_string(), v.to_string())))
            .collect()
    }

    fn method(&self) -> String {
        self.inner.req_header().method.to_string()
    }

    fn get_query_param(&self, name: &str) -> Option<String> {
        self.inner.req_header().uri.query().and_then(|query| {
            // Parse query string manually: "key1=value1&key2=value2"
            // Case-sensitive key matching per RFC 3986
            query.split('&').find_map(|pair| {
                let mut parts = pair.splitn(2, '=');
                let key = parts.next()?;
                if key == name {
                    parts.next().map(|v| v.to_string())
                } else {
                    None
                }
            })
        })
    }

    fn get_cookie(&self, name: &str) -> Option<String> {
        self.inner
            .req_header()
            .headers
            .get("Cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';').find_map(|pair| {
                    let mut parts = pair.trim().splitn(2, '=');
                    let key = parts.next()?;
                    if key == name {
                        parts.next().map(|v| v.to_string())
                    } else {
                        None
                    }
                })
            })
    }

    fn get_path(&self) -> &str {
        self.inner.req_header().uri.path()
    }

    fn get_query(&self) -> Option<String> {
        self.inner.req_header().uri.query().map(|s| s.to_string())
    }

    fn get_method(&self) -> &str {
        self.inner.req_header().method.as_str()
    }

    fn get_ctx_var(&self, key: &str) -> Option<String> {
        self.ctx.get_ctx_var(key).map(|s| s.to_string())
    }

    fn set_ctx_var(&mut self, key: &str, value: &str) -> PluginSessionResult<()> {
        self.ctx.set_ctx_var(key.to_string(), value.to_string());
        Ok(())
    }

    fn remove_ctx_var(&mut self, key: &str) -> PluginSessionResult<()> {
        self.ctx.remove_ctx_var(key);
        Ok(())
    }

    fn get_path_param(&mut self, name: &str) -> Option<String> {
        // Lazy extraction: only parse on first call
        if self.ctx.path_params.is_none() {
            self.extract_path_params();
        }
        self.ctx.path_params.as_ref()?.get(name).cloned()
    }

    async fn write_response_header(
        &mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> PluginSessionResult<()> {
        let http_status = resp.status.as_u16();
        if self.ctx.request_info.is_grpc_request && http_status != 200 {
            self.grpc_response_terminated = true;
            return Self::send_grpc_error_three_frame(self.inner, self.ctx, http_status).await;
        }
        self.inner
            .write_response_header(resp, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)
    }

    fn write_response_header_boxed<'b>(
        &'b mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PluginSessionResult<()>> + Send + 'b>> {
        Box::pin(async move {
            let http_status = resp.status.as_u16();
            if self.ctx.request_info.is_grpc_request && http_status != 200 {
                self.grpc_response_terminated = true;
                return Self::send_grpc_error_three_frame(self.inner, self.ctx, http_status).await;
            }
            self.inner
                .write_response_header(resp, end_of_stream)
                .await
                .map_err(|e| Box::new(e) as PluginSessionError)
        })
    }

    fn set_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        if let Some(resp) = &mut self.response_header {
            resp.insert_header(name.to_string(), value.to_string())
                .map_err(|e| Box::new(e) as PluginSessionError)?;
        } else {
            // Queue for later application
            self.ctx
                .response_headers_to_add
                .push((name.to_string(), value.to_string()));
        }
        Ok(())
    }

    fn append_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        if let Some(resp) = &mut self.response_header {
            resp.append_header(name.to_string(), value.to_string())
                .map_err(|e| Box::new(e) as PluginSessionError)?;
        } else {
            // Queue for later application
            self.ctx
                .response_headers_to_add
                .push((name.to_string(), value.to_string()));
        }
        Ok(())
    }

    fn remove_response_header(&mut self, name: &str) -> PluginSessionResult<()> {
        if let Some(resp) = &mut self.response_header {
            resp.remove_header(name);
        }
        Ok(())
    }

    fn response_header_names(&self) -> Vec<String> {
        match &self.response_header {
            Some(resp) => resp.headers.keys().map(|k| k.as_str().to_string()).collect(),
            None => Vec::new(),
        }
    }

    fn get_response_header(&self, name: &str) -> Option<String> {
        self.response_header
            .as_ref()
            .and_then(|resp| resp.headers.get(name))
            .and_then(|value| value.to_str().ok())
            .map(|s| s.to_string())
    }

    fn get_response_status(&self) -> Option<u16> {
        self.response_header.as_ref().map(|h| h.status.as_u16())
    }

    fn set_response_status(&mut self, status: u16) -> PluginSessionResult<()> {
        if let Some(resp) = &mut self.response_header {
            let status_code = http::StatusCode::from_u16(status).map_err(|_| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Invalid status code: {}", status),
                )) as PluginSessionError
            })?;
            resp.set_status(status_code)
                .map_err(|e| Box::new(e) as PluginSessionError)?;
        }
        Ok(())
    }

    fn set_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        self.inner
            .req_header_mut()
            .insert_header(name.to_string(), value.to_string())
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        Ok(())
    }

    fn append_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        let existing = self.header_value(name);
        let new_value = if let Some(ref current) = existing {
            format!("{},{}", current, value)
        } else {
            value.to_string()
        };
        self.set_request_header(name, &new_value)
    }

    fn remove_request_header(&mut self, name: &str) -> PluginSessionResult<()> {
        self.inner.req_header_mut().remove_header(name);
        Ok(())
    }

    fn set_upstream_uri(&mut self, uri: &str) -> PluginSessionResult<()> {
        let parsed_uri = uri.parse::<Uri>().map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner.req_header_mut().set_uri(parsed_uri);
        Ok(())
    }

    fn set_upstream_host(&mut self, host: &str) -> PluginSessionResult<()> {
        self.inner
            .req_header_mut()
            .insert_header("Host".to_string(), host.to_string())
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        Ok(())
    }

    fn set_upstream_method(&mut self, method: &str) -> PluginSessionResult<()> {
        let parsed_method = method
            .parse::<http::Method>()
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner.req_header_mut().set_method(parsed_method);
        Ok(())
    }

    async fn write_response_body(&mut self, body: Option<Bytes>, end_of_stream: bool) -> PluginSessionResult<()> {
        if self.grpc_response_terminated {
            // trailers-only response already sent; discard body from HTTP plugin
            return Ok(());
        }
        self.inner
            .write_response_body(body, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)
    }

    async fn shutdown(&mut self) {
        if self.ctx.request_info.is_grpc_request {
            // gRPC streams are terminated by END_STREAM on the HEADERS frame.
            // Calling shutdown() would send RST_STREAM on the H2 stream, which
            // the client interprets as Internal(13) regardless of any grpc-status
            // already written.
            return;
        }
        self.inner.shutdown().await;
    }

    fn client_addr(&self) -> &str {
        &self.ctx.request_info.client_addr
    }
    fn remote_addr(&self) -> &str {
        &self.ctx.request_info.remote_addr
    }

    fn set_remote_addr(&mut self, addr: &str) -> PluginSessionResult<()> {
        self.ctx.request_info.remote_addr = addr.to_string();
        Ok(())
    }

    fn client_cert_info(&self) -> Option<crate::types::ctx::ClientCertInfo> {
        self.ctx.request_info.client_cert_info.clone()
    }

    fn ctx(&self) -> &EdgionHttpContext {
        self.ctx
    }

    fn add_error(&mut self, status: crate::types::EdgionStatus) {
        self.ctx.add_error(status);
    }

    fn discover_protocol(&self) -> String {
        self.ctx.request_info.discover_protocol.clone().unwrap_or_else(|| {
            if self.ctx.request_info.is_grpc_request {
                "grpc".to_string()
            } else {
                "http".to_string()
            }
        })
    }

    fn grpc_service(&self) -> Option<String> {
        self.ctx.request_info.grpc_service.clone()
    }

    fn grpc_method(&self) -> Option<String> {
        self.ctx.request_info.grpc_method.clone()
    }

    fn push_plugin_ref(&mut self, key: String) {
        self.ctx.push_plugin_ref(key);
    }

    fn pop_plugin_ref(&mut self) {
        self.ctx.pop_plugin_ref();
    }

    fn plugin_ref_depth(&self) -> usize {
        self.ctx.plugin_ref_depth()
    }

    fn has_plugin_ref(&self, key: &str) -> bool {
        self.ctx.has_plugin_ref(key)
    }

    fn push_edgion_plugins_log(&mut self, log: EdgionPluginsLog) {
        self.ctx.pending_edgion_plugins_logs.push(log);
    }

    fn start_edgion_plugins_log(&mut self, name: String) -> EdgionPluginsLogToken {
        let idx = self.ctx.pending_edgion_plugins_logs.len();
        let depth = self.ctx.plugin_ref_depth();
        self.ctx
            .pending_edgion_plugins_logs
            .push(EdgionPluginsLog { name, logs: Vec::new() });
        EdgionPluginsLogToken::new(idx, depth)
    }

    fn push_to_edgion_plugins_log(&mut self, token: &EdgionPluginsLogToken, log: PluginLog) {
        debug_assert_eq!(
            self.ctx.plugin_ref_depth(),
            token.depth(),
            "EdgionPluginsLogToken used in wrong scope: expected depth {}, got {}",
            token.depth(),
            self.ctx.plugin_ref_depth()
        );
        if let Some(edgion_log) = self.ctx.pending_edgion_plugins_logs.get_mut(token.idx()) {
            edgion_log.logs.push(log);
        }
    }

    async fn key_get(&self, key: &KeyGet) -> Option<String> {
        match key {
            // ---- Sync variants: zero-cost, no actual await ----
            KeyGet::ClientIp => {
                let ip = self.remote_addr();
                if ip.is_empty() {
                    None
                } else {
                    Some(ip.to_string())
                }
            }
            KeyGet::Header { name } => self.header_value(name),
            KeyGet::Cookie { name } => self.get_cookie(name),
            KeyGet::Query { name } => self.get_query_param(name),
            KeyGet::Path => Some(self.get_path().to_string()),
            KeyGet::Method => Some(self.get_method().to_string()),
            KeyGet::Ctx { name } => self.get_ctx_var(name),
            KeyGet::ClientIpAndPath => {
                let ip = self.remote_addr();
                if ip.is_empty() {
                    None
                } else {
                    Some(format!("{}:{}", ip, self.get_path()))
                }
            }

            // ---- Async variant: calls external webhook service ----
            KeyGet::Webhook { webhook_ref, extract } => self.resolve_webhook(webhook_ref, extract).await,
        }
    }

    fn key_get_local(&self, key: &KeyGet) -> Option<String> {
        match key {
            KeyGet::ClientIp => {
                let ip = self.remote_addr();
                if ip.is_empty() {
                    None
                } else {
                    Some(ip.to_string())
                }
            }
            KeyGet::Header { name } => self.header_value(name),
            KeyGet::Cookie { name } => self.get_cookie(name),
            KeyGet::Query { name } => self.get_query_param(name),
            KeyGet::Path => Some(self.get_path().to_string()),
            KeyGet::Method => Some(self.get_method().to_string()),
            KeyGet::Ctx { name } => self.get_ctx_var(name),
            KeyGet::ClientIpAndPath => {
                let ip = self.remote_addr();
                if ip.is_empty() {
                    None
                } else {
                    Some(format!("{}:{}", ip, self.get_path()))
                }
            }
            KeyGet::Webhook { .. } => {
                tracing::warn!(
                    "key_get_local called with Webhook variant; returning None (use key_get for async resolution)"
                );
                None
            }
        }
    }

    fn key_set(&mut self, key: &KeySet, value: Option<String>) -> PluginSessionResult<()> {
        match (key, value) {
            (KeySet::Header { name }, Some(v)) => self.set_request_header(name, &v),
            (KeySet::Header { name }, None) => self.remove_request_header(name),
            (KeySet::ResponseHeader { name }, Some(v)) => self.set_response_header(name, &v),
            (KeySet::ResponseHeader { name }, None) => self.remove_response_header(name),
            (KeySet::Cookie { name }, Some(v)) => {
                // Set cookie via Set-Cookie header
                self.set_response_header("Set-Cookie", &format!("{}={}", name, v))
            }
            (KeySet::Cookie { name }, None) => {
                // Remove cookie by setting Max-Age=0
                self.set_response_header("Set-Cookie", &format!("{}=; Max-Age=0", name))
            }
            (KeySet::Ctx { name }, Some(v)) => self.set_ctx_var(name, &v),
            (KeySet::Ctx { name }, None) => self.remove_ctx_var(name),
        }
    }

    fn set_direct_endpoint(&mut self, info: DirectEndpointPreset) {
        self.ctx.direct_endpoint = Some(info);
    }

    fn set_internal_jump(&mut self, info: InternalJumpPreset) {
        self.ctx.internal_jump = Some(info);
    }

    fn set_external_jump(&mut self, info: ExternalJumpPreset) {
        self.ctx.external_jump = Some(info);
    }

    fn set_mirror_state(&mut self, state: MirrorState) {
        self.ctx.mirror_state = Some(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_path_params_single_param() {
        let pattern = "/api/:uid/profile";
        let actual = "/api/123/profile";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("uid"), Some(&"123".to_string()));
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn test_parse_path_params_multiple_params() {
        let pattern = "/users/:uid/posts/:post_id";
        let actual = "/users/456/posts/789";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("uid"), Some(&"456".to_string()));
        assert_eq!(params.get("post_id"), Some(&"789".to_string()));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_parse_path_params_no_params() {
        let pattern = "/api/v1/users";
        let actual = "/api/v1/users";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_path_params_escaped_colon() {
        // :: is escaped colon, not a parameter
        let pattern = "/api/::version/data";
        let actual = "/api/:v1/data";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_path_params_param_at_end() {
        let pattern = "/users/:id";
        let actual = "/users/999";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("id"), Some(&"999".to_string()));
    }

    #[test]
    fn test_parse_path_params_prefix_match() {
        // For prefix match, actual path may have more segments
        let pattern = "/api/:version";
        let actual = "/api/v2/users/123";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("version"), Some(&"v2".to_string()));
    }

    #[test]
    fn test_parse_path_params_empty_param_name() {
        // Empty param name should be skipped
        let pattern = "/api/:/data";
        let actual = "/api/test/data";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_path_params_special_characters_in_value() {
        let pattern = "/search/:query";
        let actual = "/search/hello%20world";
        let params = PingoraSessionAdapter::parse_path_params(pattern, actual);

        assert_eq!(params.get("query"), Some(&"hello%20world".to_string()));
    }
}
