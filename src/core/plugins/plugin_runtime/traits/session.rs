//! PluginSession trait - shared across all filter stages

use crate::core::plugins::plugin_runtime::log::{EdgionPluginsLog, EdgionPluginsLogToken, PluginLog};
use crate::types::EdgionHttpContext;
use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

pub type PluginSessionError = Box<dyn std::error::Error + Send + Sync>;
pub type PluginSessionResult<T> = Result<T, PluginSessionError>;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait PluginSession: Send {
    /// Get a request header value by name
    fn header_value(&self, name: &str) -> Option<String>;

    /// Get the HTTP method (returns owned String for compatibility)
    fn method(&self) -> String;

    // ========== Condition evaluation methods (read-only) ==========

    /// Get a query parameter value by name
    fn get_query_param(&self, name: &str) -> Option<String>;

    /// Get a cookie value by name
    fn get_cookie(&self, name: &str) -> Option<String>;

    /// Get the request path
    fn get_path(&self) -> &str;

    /// Get the HTTP method as &str (more efficient than method())
    fn get_method(&self) -> &str;

    /// Get a context variable by key (set by plugins like KeySet)
    fn get_ctx_var(&self, key: &str) -> Option<String>;

    /// Set a context variable (for plugins to pass data downstream)
    fn set_ctx_var(&mut self, key: &str, value: &str) -> PluginSessionResult<()>;

    async fn write_response_header(
        &mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> PluginSessionResult<()>;

    fn write_response_header_boxed<'a>(
        &'a mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PluginSessionResult<()>> + Send + 'a>>;

    /// Set a response header (will be added when write_response_header is called)
    fn set_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Append a value to an existing response header (e.g., Vary: Origin)
    fn append_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Remove a response header (for ResponseHeaderModifier)
    fn remove_response_header(&mut self, name: &str) -> PluginSessionResult<()>;

    /// Set a request header (for upstream)
    fn set_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Append a value to an existing request header (e.g., X-Forwarded-For)
    fn append_request_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()>;

    /// Remove a request header (e.g., hide credentials)
    fn remove_request_header(&mut self, name: &str) -> PluginSessionResult<()>;

    /// Set the upstream URI (for proxy rewrite)
    fn set_upstream_uri(&mut self, uri: &str) -> PluginSessionResult<()>;

    /// Set the upstream host (for proxy rewrite)
    fn set_upstream_host(&mut self, host: &str) -> PluginSessionResult<()>;

    /// Set the upstream HTTP method (for proxy rewrite)
    fn set_upstream_method(&mut self, method: &str) -> PluginSessionResult<()>;

    async fn write_response_body(&mut self, body: Option<Bytes>, end_of_stream: bool) -> PluginSessionResult<()>;

    async fn shutdown(&mut self);

    /// Get client IP address (TCP direct connection, without port)
    fn client_addr(&self) -> &str;

    /// Get remote address (real client IP, extracted from proxy headers)
    fn remote_addr(&self) -> &str;

    /// Get reference to EdgionHttpContext (for access log generation)
    fn ctx(&self) -> &EdgionHttpContext;

    /// Track nested plugin references to prevent cycles
    fn push_plugin_ref(&mut self, key: String);

    /// Remove last tracked plugin reference
    fn pop_plugin_ref(&mut self);

    /// Current depth of nested plugin references
    fn plugin_ref_depth(&self) -> usize;

    /// Check whether reference key already exists in stack
    fn has_plugin_ref(&self, key: &str) -> bool;

    /// Push EdgionPlugins execution log to pending list (for ExtensionRef)
    fn push_edgion_plugins_log(&mut self, log: EdgionPluginsLog);

    /// Start a new EdgionPluginsLog and return a token for safe log pushing.
    ///
    /// This creates an empty EdgionPluginsLog entry in the pending list and returns
    /// a token that can be used with `push_to_edgion_plugins_log` to append logs.
    /// The token records the current depth to prevent misuse in nested scopes.
    fn start_edgion_plugins_log(&mut self, name: String) -> EdgionPluginsLogToken;

    /// Push a PluginLog to the EdgionPluginsLog identified by the token.
    ///
    /// # Panics
    /// In debug builds, panics if the current depth doesn't match the token's depth,
    /// indicating the token is being used in the wrong scope.
    fn push_to_edgion_plugins_log(&mut self, token: &EdgionPluginsLogToken, log: PluginLog);
}
