use super::transport::Outcome;
use super::{failure, GrpcTranscodingPlan};
use crate::{
    ctx::ResponseStreamKind,
    plugins::runtime::traits::{PluginSession, StreamingResponseEvent, StreamingResponseProcessor},
    sse::window::{RawWindow, WindowDecision, WindowOutput, WindowRecord},
};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::grpc_transcoding::{CappedWriter, GrpcJsonStreamFormat};
use http::HeaderMap;
use pingora_http::ResponseHeader;
use pingora_proxy::{ResponseBodySink, ResponseHeadReplacement};
use std::{
    io::Write,
    sync::{atomic::Ordering, Arc},
};

#[cfg(not(test))]
fn callback_writer() -> CappedWriter {
    CappedWriter::new(1024 * 1024)
}
#[cfg(test)]
use tests::callback_writer;

pub(crate) const ERROR_JSON: &[u8] = b"{\"error\":\"gRPC upstream exchange failed\"}";
fn status(headers: &HeaderMap) -> Result<Option<u8>, ()> {
    let mut all = headers.get_all("grpc-status").iter();
    let Some(value) = all.next() else {
        return Ok(None);
    };
    if all.next().is_some() {
        return Err(());
    }
    let text = value.to_str().map_err(|_| ())?;
    if text.is_empty() || text.len() > 2 || !text.bytes().all(|c| c.is_ascii_digit()) {
        return Err(());
    }
    let value = text.parse::<u8>().map_err(|_| ())?;
    if value > 16 || text != value.to_string() {
        return Err(());
    }
    Ok(Some(value))
}

/// Pin only validated transport status before mutable trailer companions run.
/// An empty evidence map deliberately preserves missing/invalid status failure.
pub(crate) fn status_evidence(headers: &HeaderMap) -> HeaderMap {
    let mut evidence = HeaderMap::new();
    if let Ok(Some(code)) = status(headers) {
        const CODES: [&str; 17] = [
            "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
        ];
        evidence.insert("grpc-status", http::HeaderValue::from_static(CODES[code as usize]));
    }
    evidence
}
fn mapped_status(code: u8) -> u16 {
    match code {
        0 => 200,
        1 => 499,
        3 | 9 | 11 => 400,
        4 => 504,
        5 => 404,
        6 | 10 => 409,
        7 => 403,
        8 => 429,
        12 => 501,
        14 => 503,
        16 => 401,
        _ => 502,
    }
}
fn media(format: GrpcJsonStreamFormat, streaming: bool) -> &'static str {
    if !streaming {
        "application/json"
    } else {
        match format {
            GrpcJsonStreamFormat::Ndjson => "application/x-ndjson",
            GrpcJsonStreamFormat::Sse => "text/event-stream",
        }
    }
}
pub(super) fn json_headers_valid(header: &ResponseHeader, streaming: bool, format: GrpcJsonStreamFormat) -> bool {
    header
        .headers
        .get("content-type")
        .is_some_and(|v| v == media(format, streaming))
        && ![
            "content-length",
            "content-encoding",
            "grpc-encoding",
            "grpc-status",
            "grpc-message",
            "trailer",
        ]
        .iter()
        .any(|n| header.headers.contains_key(*n))
}
fn terminal(format: GrpcJsonStreamFormat, code: Option<u8>) -> Vec<u8> {
    match (format, code) {
        (GrpcJsonStreamFormat::Ndjson, Some(0)) => b"{\"type\":\"end\"}\n".to_vec(),
        (GrpcJsonStreamFormat::Sse, Some(0)) => b"event: rpc_end\ndata: {}\n\n".to_vec(),
        (GrpcJsonStreamFormat::Ndjson, Some(code)) => {
            format!("{{\"type\":\"error\",\"grpcStatus\":{code},\"message\":\"gRPC upstream exchange failed\"}}\n")
                .into_bytes()
        }
        (GrpcJsonStreamFormat::Sse, Some(code)) => format!(
            "event: rpc_error\ndata: {{\"grpcStatus\":{code},\"message\":\"gRPC upstream exchange failed\"}}\n\n"
        )
        .into_bytes(),
        (GrpcJsonStreamFormat::Ndjson, None) => {
            b"{\"type\":\"error\",\"message\":\"gRPC upstream exchange failed\"}\n".to_vec()
        }
        (GrpcJsonStreamFormat::Sse, None) => {
            b"event: rpc_error\ndata: {\"message\":\"gRPC upstream exchange failed\"}\n\n".to_vec()
        }
    }
}

pub(crate) struct GrpcResponseProcessor {
    plan: Arc<GrpcTranscodingPlan>,
    prefix: [u8; 5],
    prefix_len: usize,
    expected: usize,
    received: usize,
    window: RawWindow,
    unary: Vec<u8>,
    messages: usize,
    header_status: Result<Option<u8>, ()>,
    finished: bool,
}
impl GrpcResponseProcessor {
    pub(crate) fn new(plan: Arc<GrpcTranscodingPlan>) -> Self {
        Self {
            plan,
            prefix: [0; 5],
            prefix_len: 0,
            expected: 0,
            received: 0,
            window: RawWindow::new(),
            unary: Vec::new(),
            messages: 0,
            header_status: Ok(None),
            finished: false,
        }
    }
    fn feed(&mut self, chunk: Bytes) -> Result<Vec<u8>, ()> {
        let mut output = None;
        let mut pos = 0;
        let mut records = 0usize;
        while pos < chunk.len() {
            if !self.plan.response_valid() {
                return Err(());
            }
            if self.prefix_len < 5 {
                let n = (5 - self.prefix_len).min(chunk.len() - pos);
                self.prefix[self.prefix_len..self.prefix_len + n].copy_from_slice(&chunk[pos..pos + n]);
                self.prefix_len += n;
                pos += n;
                if self.prefix_len != 5 {
                    break;
                }
                self.expected = u32::from_be_bytes(self.prefix[1..5].try_into().map_err(|_| ())?) as usize;
                if self.prefix[0] != 0
                    || self.expected > self.plan.compiled.limits.response
                    || (!self.plan.streaming() && self.messages != 0)
                {
                    return Err(());
                }
                self.received = 0;
                if !self.plan.streaming() {
                    self.unary = Vec::with_capacity(self.expected);
                }
            }
            let n = (self.expected - self.received).min(chunk.len() - pos);
            if self.plan.streaming() {
                if n != 0 {
                    self.window
                        .retain(WindowRecord::binary(chunk.slice(pos..pos + n)))
                        .map_err(|_| ())?;
                }
            } else {
                self.unary.extend_from_slice(&chunk[pos..pos + n]);
            }
            pos += n;
            self.received += n;
            if self.received == self.expected {
                records += 1;
                if records > 256 {
                    return Err(());
                }
                self.messages = self.messages.checked_add(1).ok_or(())?;
                if self.plan.streaming() {
                    let payload = self.window.binary_bytes().map_err(|_| ())?;
                    let json = self
                        .plan
                        .binding
                        .decode(&payload, self.plan.compiled.limits, Some(self.plan.compiled.format))
                        .map_err(|_| ())?;
                    let WindowOutput::Replace(json) = self
                        .window
                        .resolve(WindowDecision::Replace(Bytes::from(json)))
                        .map_err(|_| ())?
                    else {
                        return Err(());
                    };
                    output
                        .get_or_insert_with(callback_writer)
                        .write_all(&json)
                        .map_err(|_| ())?;
                    self.window.start_next().map_err(|_| ())?;
                }
                self.prefix_len = 0;
                self.expected = 0;
                self.received = 0;
            }
        }
        Ok(output.map_or_else(Vec::new, CappedWriter::into_bytes))
    }
    fn settle(&mut self, code: Option<u8>) -> pingora_core::Result<()> {
        if self.finished {
            return Err(failure());
        }
        self.finished = true;
        let valid = self.plan.response_valid() && self.prefix_len == 0;
        if let Some(code) = code {
            self.plan.record_status(code);
        }
        if self.prefix_len != 0 {
            self.plan.record(Outcome::InvalidFraming);
        }
        if valid {
            self.plan.record(match code {
                Some(0) => Outcome::Success,
                Some(_) => Outcome::RpcStatus,
                None => Outcome::MissingStatus,
            });
        }
        if self.plan.streaming() {
            let _ = self.window.finish(WindowDecision::Discard);
            let code = if valid { code } else { None };
            let output = terminal(self.plan.compiled.format, code);
            if output.len() > self.plan.compiled.limits.json {
                return Err(failure());
            }
            *self.plan.terminal.lock() = Some(self.plan.output(output).map_err(|_| failure())?);
            self.plan.release_reservation();
            return Ok(());
        }
        if !valid || code.is_none() || code == Some(0) && self.messages != 1 {
            if code == Some(0) && self.messages != 1 {
                self.plan.record(Outcome::InvalidFraming);
            }
            self.unary.clear();
            return Err(failure());
        }
        let code = code.ok_or_else(failure)?;
        let output = if code == 0 {
            self.plan
                .binding
                .decode(&self.unary, self.plan.compiled.limits, None)
                .map_err(|_| {
                    self.plan.record(Outcome::InvalidFraming);
                    failure()
                })?
        } else {
            ERROR_JSON.to_vec()
        };
        self.unary = Vec::new();
        if !self.plan.response_valid() || output.len() > self.plan.compiled.limits.json {
            return Err(failure());
        }
        let body = self.plan.output(output).map_err(|_| failure())?;
        let mut header = self.plan.template.lock().take().ok_or_else(failure)?;
        // Replacement bypasses the fork's origin-header version normalization.
        // H1 writers require HTTP/1.1; H2 writers ignore this status-line version.
        header.set_version(http::Version::HTTP_11);
        header.set_status(mapped_status(code))?;
        header.insert_header("content-type", "application/json")?;
        header.insert_header("content-length", body.len().to_string())?;
        header.remove_header("transfer-encoding");
        *self.plan.ready.lock() = Some(ResponseHeadReplacement::new(Box::new(header), vec![body]));
        self.plan.release_reservation();
        Ok(())
    }
    fn abort(&mut self, sink: &mut ResponseBodySink) {
        self.release();
        if self.plan.streaming() || sink.fail_response_head(failure()).is_err() {
            // Immediate responses have no held head to fail. Terminate is the
            // canonical pump cancellation seam; missing terminal means incomplete.
            sink.terminate();
        }
    }
    fn release(&mut self) {
        self.plan.interrupted();
        self.finished = true;
        self.unary = Vec::new();
        let _ = self.window.finish(WindowDecision::Discard);
        self.plan.clear_output();
    }
}
#[async_trait]
impl StreamingResponseProcessor for GrpcResponseProcessor {
    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        self.header_status = status(&response.headers);
        let valid = response.status.as_u16() == 200
            && response
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    let v = v.split(';').next().unwrap_or("").trim();
                    v == "application/grpc" || v == "application/grpc+proto"
                })
            && ["content-encoding", "grpc-encoding"]
                .iter()
                .all(|n| response.headers.get(*n).is_none_or(|v| v == "identity"));
        self.plan.invalid_response.store(!valid, Ordering::Release);
        if !valid {
            self.plan.record(Outcome::InvalidMedia);
        }
        for name in [
            "content-length",
            "content-encoding",
            "grpc-encoding",
            "grpc-accept-encoding",
            "grpc-status",
            "grpc-message",
            "grpc-status-details-bin",
            "trailer",
        ] {
            response.remove_header(name);
        }
        let _ = response.insert_header("content-type", media(self.plan.compiled.format, self.plan.streaming()));
    }
    fn changes_body_length(&self) -> bool {
        true
    }
    fn release_inflight(&mut self) {
        self.release();
    }
    fn finalize_incomplete(&mut self, _: &'static str, _: &'static str) {
        self.release();
    }
    fn on_terminal_preempted(&mut self, _: crate::response::terminal::ResponseTerminalCause) {
        self.release();
    }
    async fn on_actual_upstream_trailers(
        &mut self,
        trailers: Option<&HeaderMap>,
        header_end_stream: bool,
    ) -> pingora_core::Result<()> {
        if self.finished {
            return Ok(());
        }
        let code = match trailers {
            Some(trailers) if self.header_status == Ok(None) => status(trailers).ok().flatten(),
            Some(_) => None,
            None if header_end_stream => self.header_status.ok().flatten(),
            None => None,
        };
        self.settle(code)
    }
    async fn on_event(
        &mut self,
        chunk: &mut Option<Bytes>,
        event: StreamingResponseEvent,
        _: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        _: &mut dyn PluginSession,
    ) {
        if self.finished {
            *chunk = None;
            return;
        }
        let input = chunk.take();
        let output = if !self.plan.response_valid() {
            Err(())
        } else {
            match input {
                Some(input) => self.feed(input),
                None => Ok(Vec::new()),
            }
        };
        match output {
            Ok(output) if !output.is_empty() => match self.plan.output(output) {
                Ok(output) => *chunk = Some(output),
                Err(_) => {
                    self.abort(sink);
                }
            },
            Ok(_) => {}
            Err(_) => {
                self.plan.record(Outcome::InvalidFraming);
                if self.plan.streaming() {
                    match self
                        .settle(None)
                        .and_then(|_| self.plan.terminal.lock().take().ok_or_else(failure))
                    {
                        Ok(error) => {
                            *chunk = Some(error);
                            sink.terminate();
                        }
                        Err(_) => {
                            self.abort(sink);
                        }
                    }
                } else {
                    self.abort(sink);
                }
            }
        }
        // Real trailers own success. Pre-trailer and no-trailers boundaries
        // never fabricate a grpc-status; the core finalizer supplies evidence.
        let _ = event;
    }
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        eos: bool,
        kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        self.on_event(
            chunk,
            StreamingResponseEvent::Data { end_of_stream: eos },
            kind,
            sink,
            session,
        )
        .await;
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/grpc_json_transcoder/response_test.rs"]
mod tests;
