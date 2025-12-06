use async_trait::async_trait;
use bytes::Bytes;
use http::Uri;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use parking_lot::Mutex;
use std::sync::Arc;

use super::traits::{FilterSession, FilterSessionError, FilterSessionResult};
use crate::types::filters::FilterRunningStage;

pub struct PingoraSessionAdapter<'a> {
    inner: &'a mut Session,
    stage: FilterRunningStage,
    response_headers: Arc<Mutex<Vec<(String, String, bool)>>>, // (name, value, is_append)
    request_headers_to_set: Arc<Mutex<Vec<(String, String)>>>,
    request_headers_to_remove: Arc<Mutex<Vec<String>>>,
    upstream_uri: Arc<Mutex<Option<String>>>,
    upstream_host: Arc<Mutex<Option<String>>>,
    upstream_method: Arc<Mutex<Option<String>>>,
}

impl<'a> PingoraSessionAdapter<'a> {
    pub fn new(inner: &'a mut Session, stage: FilterRunningStage) -> Self {
        Self {
            inner,
            stage,
            response_headers: Arc::new(Mutex::new(Vec::new())),
            request_headers_to_set: Arc::new(Mutex::new(Vec::new())),
            request_headers_to_remove: Arc::new(Mutex::new(Vec::new())),
            upstream_uri: Arc::new(Mutex::new(None)),
            upstream_host: Arc::new(Mutex::new(None)),
            upstream_method: Arc::new(Mutex::new(None)),
        }
    }

    /// Apply pending request header modifications
    pub fn apply_request_header_modifications(&mut self) {
        // Remove headers
        let headers_to_remove = {
            let guard = self.request_headers_to_remove.lock();
            guard.clone()
        };

        for name in headers_to_remove {
            self.inner.req_header_mut().remove_header(&name);
        }

        // Set/add headers
        let headers_to_set = {
            let guard = self.request_headers_to_set.lock();
            guard.clone()
        };

        for (name, value) in headers_to_set {
            let _ = self.inner.req_header_mut().insert_header(name, value);
        }

        // Apply upstream URI if set
        if let Some(uri_str) = self.upstream_uri.lock().as_ref() {
            if let Ok(uri) = uri_str.parse::<Uri>() {
                self.inner.req_header_mut().set_uri(uri);
            }
        }

        // Apply upstream host if set
        if let Some(host) = self.upstream_host.lock().as_ref() {
            let _ = self.inner.req_header_mut().insert_header("Host", host);
        }

        // Apply upstream method if set
        if let Some(method_str) = self.upstream_method.lock().as_ref() {
            if let Ok(method) = method_str.parse::<http::Method>() {
                self.inner.req_header_mut().set_method(method);
            }
        }
    }
}

#[async_trait]
impl<'a> FilterSession for PingoraSessionAdapter<'a> {
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
        mut resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> FilterSessionResult<()> {
        // Apply accumulated headers
        let headers_to_add = {
            let headers = self.response_headers.lock();
            headers.clone()
        };

        for (name, value, is_append) in headers_to_add {
            if is_append {
                if let Some(existing) = resp.headers.get(&name) {
                    if let Ok(existing_str) = existing.to_str() {
                        let combined = format!("{}, {}", existing_str, value);
                        resp.insert_header(name, combined)
                            .map_err(|e| Box::new(e) as FilterSessionError)?;
                        continue;
                    }
                }
            }
            resp.insert_header(name, value)
                .map_err(|e| Box::new(e) as FilterSessionError)?;
        }

        self.inner
            .write_response_header(resp, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as FilterSessionError)
    }

    fn write_response_header_boxed<'b>(
        &'b mut self,
        mut resp: Box<ResponseHeader>,
        end_of_stream: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = FilterSessionResult<()>> + Send + 'b>> {
        let headers_arc = self.response_headers.clone();
        let headers_to_add = {
            let headers = headers_arc.lock();
            headers.clone()
        };

        for (name, value, is_append) in headers_to_add {
            if is_append {
                if let Some(existing) = resp.headers.get(&name) {
                    if let Ok(existing_str) = existing.to_str() {
                        let combined = format!("{}, {}", existing_str, value);
                        if let Err(e) = resp.insert_header(name, combined) {
                            let err_msg = format!("{}", e);
                            return Box::pin(async move {
                                Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, err_msg)) as FilterSessionError)
                            });
                        }
                        continue;
                    }
                }
            }
            if let Err(e) = resp.insert_header(name, value) {
                let err_msg = format!("{}", e);
                return Box::pin(async move {
                    Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, err_msg)) as FilterSessionError)
                });
            }
        }

        Box::pin(async move {
            self.inner
                .write_response_header(resp, end_of_stream)
                .await
                .map_err(|e| Box::new(e) as FilterSessionError)
        })
    }

    fn set_response_header(&mut self, name: &str, value: &str) -> FilterSessionResult<()> {
        self.response_headers.lock().push((name.to_string(), value.to_string(), false));
        Ok(())
    }

    fn append_response_header(&mut self, name: &str, value: &str) -> FilterSessionResult<()> {
        self.response_headers.lock().push((name.to_string(), value.to_string(), true));
        Ok(())
    }

    fn set_request_header(&mut self, name: &str, value: &str) -> FilterSessionResult<()> {
        self.request_headers_to_set.lock().push((name.to_string(), value.to_string()));
        Ok(())
    }

    fn append_request_header(&mut self, name: &str, value: &str) -> FilterSessionResult<()> {
        let existing = self.header_value(name);
        let new_value = if let Some(ref current) = existing {
            format!("{}, {}", current, value)
        } else {
            value.to_string()
        };
        self.set_request_header(name, &new_value)
    }

    fn remove_request_header(&mut self, name: &str) -> FilterSessionResult<()> {
        self.request_headers_to_remove.lock().push(name.to_string());
        Ok(())
    }

    fn set_upstream_uri(&mut self, uri: &str) -> FilterSessionResult<()> {
        *self.upstream_uri.lock() = Some(uri.to_string());
        Ok(())
    }

    fn set_upstream_host(&mut self, host: &str) -> FilterSessionResult<()> {
        *self.upstream_host.lock() = Some(host.to_string());
        Ok(())
    }

    fn set_upstream_method(&mut self, method: &str) -> FilterSessionResult<()> {
        *self.upstream_method.lock() = Some(method.to_string());
        Ok(())
    }

    async fn write_response_body(
        &mut self,
        body: Option<Bytes>,
        end_of_stream: bool,
    ) -> FilterSessionResult<()> {
        self.inner
            .write_response_body(body, end_of_stream)
            .await
            .map_err(|e| Box::new(e) as FilterSessionError)
    }

    async fn shutdown(&mut self) {
        self.inner.shutdown().await;
    }

    fn get_stage(&self) -> FilterRunningStage {
        self.stage
    }
}
