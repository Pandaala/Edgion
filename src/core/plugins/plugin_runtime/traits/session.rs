//! PluginSession trait - shared across all filter stages

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use crate::types::EdgionHttpContext;

pub type PluginSessionError = Box<dyn std::error::Error + Send + Sync>;
pub type PluginSessionResult<T> = Result<T, PluginSessionError>;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait PluginSession: Send {
    fn header_value(&mut self, name: &str) -> Option<String>;

    fn method(&self) -> String;

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

    async fn write_response_body(
        &mut self,
        body: Option<Bytes>,
        end_of_stream: bool,
    ) -> PluginSessionResult<()>;

    async fn shutdown(&mut self);

    /// Get client address (TCP direct connection address)
    fn client_addr(&self) -> &str;

    /// Get remote address (real client IP, extracted from proxy headers)
    fn remote_addr(&self) -> &str;

    /// Get reference to EdgionHttpContext (for access log generation)
    fn ctx(&self) -> &EdgionHttpContext;
}

