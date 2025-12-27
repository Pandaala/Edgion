use async_trait::async_trait;
use bytes::Bytes;
use http::Uri;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

use crate::types::EdgionHttpContext;
use crate::types::filters::PluginRunningResult;
use super::log::PluginLog;
use super::filters::{PluginSession, PluginSessionError, PluginSessionResult};

pub struct PingoraSessionAdapter<'a> {
    inner: &'a mut Session,
    ctx: &'a mut EdgionHttpContext,
    response_header: Option<&'a mut ResponseHeader>,
}

impl<'a> PingoraSessionAdapter<'a> {
    #[inline]
    pub fn new(inner: &'a mut Session, ctx: &'a mut EdgionHttpContext) -> Self {
        Self { inner, ctx, response_header: None }
    }

    #[inline]
    pub fn with_response_header(
        inner: &'a mut Session,
        ctx: &'a mut EdgionHttpContext,
        response_header: &'a mut ResponseHeader,
    ) -> Self {
        Self { inner, ctx, response_header: Some(response_header) }
    }

    #[inline]
    pub fn push_plugin_log(&mut self, log: PluginLog) {
        self.ctx.plugin_logs.push(log);
    }

    #[inline]
    pub fn set_terminate(&mut self) {
        self.ctx.plugin_running_result = PluginRunningResult::ErrTerminateRequest;
    }
}

#[async_trait]
impl<'a> PluginSession for PingoraSessionAdapter<'a> {
    fn header_value(&mut self, name: &str) -> Option<String> {
        self.inner
            .req_header()
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|s| s.to_string())
    }

    fn method(&self) -> String {
        self.inner.req_header().method.to_string()
    }

    async fn write_response_header(
        &mut self,
        resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> PluginSessionResult<()> {
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
        }
        Ok(())
    }

    fn append_response_header(&mut self, name: &str, value: &str) -> PluginSessionResult<()> {
        if let Some(resp) = &mut self.response_header {
            resp.append_header(name.to_string(), value.to_string())
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
            format!("{}, {}", current, value)
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
        let parsed_uri = uri.parse::<Uri>()
            .map_err(|e| Box::new(e) as PluginSessionError)?;
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
        let parsed_method = method.parse::<http::Method>()
            .map_err(|e| Box::new(e) as PluginSessionError)?;
        self.inner.req_header_mut().set_method(parsed_method);
        Ok(())
    }

    async fn write_response_body(
        &mut self,
        body: Option<Bytes>,
        end_of_stream: bool,
    ) -> PluginSessionResult<()> {
        self.inner
            .write_response_body(body, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as PluginSessionError)
    }

    async fn shutdown(&mut self) {
        self.inner.shutdown().await;
    }

    fn client_addr(&self) -> &str { &self.ctx.request_info.client_addr }
    fn remote_addr(&self) -> &str { &self.ctx.request_info.remote_addr }
}
