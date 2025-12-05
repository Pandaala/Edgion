use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

use crate::types::filters::{FilterConf, FilterRunningResult, FilterRunningStage};

pub type FilterSessionError = Box<dyn std::error::Error + Send + Sync>;
pub type FilterSessionResult<T> = Result<T, FilterSessionError>;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait FilterSession: Send {
    fn header_value(&mut self, name: &str) -> Option<String>;

    fn method(&self) -> String;

    async fn write_response_header(
        &mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> FilterSessionResult<()>;

    fn write_response_header_boxed<'a>(
        &'a mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = FilterSessionResult<()>> + Send + 'a>>;

    /// Set a response header (will be added when write_response_header is called)
    fn set_response_header(&mut self, name: &str, value: &str) -> FilterSessionResult<()>;

    /// Append a value to an existing response header (e.g., Vary: Origin)
    fn append_response_header(&mut self, name: &str, value: &str) -> FilterSessionResult<()>;

    /// Set a request header (for upstream)
    fn set_request_header(&mut self, name: &str, value: &str) -> FilterSessionResult<()>;

    /// Append a value to an existing request header (e.g., X-Forwarded-For)
    fn append_request_header(&mut self, name: &str, value: &str) -> FilterSessionResult<()>;

    /// Remove a request header (e.g., hide credentials)
    fn remove_request_header(&mut self, name: &str) -> FilterSessionResult<()>;

    /// Set the upstream URI (for proxy rewrite)
    fn set_upstream_uri(&mut self, uri: &str) -> FilterSessionResult<()>;

    /// Set the upstream host (for proxy rewrite)
    fn set_upstream_host(&mut self, host: &str) -> FilterSessionResult<()>;

    /// Set the upstream HTTP method (for proxy rewrite)
    fn set_upstream_method(&mut self, method: &str) -> FilterSessionResult<()>;

    async fn write_response_body(
        &mut self,
        body: Option<Bytes>,
        end_of_stream: bool,
    ) -> FilterSessionResult<()>;

    async fn shutdown(&mut self);

    fn get_stage(&self) -> FilterRunningStage;

    /// Add a miscellaneous log message
    /// Filters can use this to record runtime logs
    fn add_misc_log(&mut self, log: String) -> FilterSessionResult<()>;
}

#[async_trait]
pub trait Filter: Send + Sync {
    fn name(&self) -> &str;

    async fn run(
        &self,
        session: &mut dyn FilterSession,
    ) -> FilterRunningResult;

    fn get_stages(&self) -> Vec<FilterRunningStage>;

    fn check_schema(&self, _conf: &FilterConf);
}
