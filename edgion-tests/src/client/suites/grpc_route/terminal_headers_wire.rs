//! Full shared-stack terminal gRPC envelope coverage (ARCH-003).
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use bytes::Bytes;
use http::HeaderMap;
use parking_lot::Mutex;
use prost::Message;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

const DEADLINE: Duration = Duration::from_secs(10);
const MAX_BYTES: usize = 64 * 1024;
const MAX_FRAMES: usize = 64;

pub struct GrpcTerminalHeadersWireTestSuite;

#[derive(Clone, Copy, Debug)]
struct FrameMeta {
    kind: u8,
    flags: u8,
    stream: u32,
}

/// Observe framing only; h2 remains the sole semantic decoder. Caps cover every
/// incoming byte/frame, including control frames, not just the tested stream.
#[derive(Default)]
struct FrameScan {
    header: [u8; 9],
    header_used: usize,
    remaining: usize,
    pending: Option<FrameMeta>,
    bytes: usize,
    frames: usize,
    stream_frames: Vec<FrameMeta>,
    failure: Option<&'static str>,
}

impl FrameScan {
    fn fail(&mut self, reason: &'static str) {
        self.failure.get_or_insert(reason);
    }

    fn observe(&mut self, mut bytes: &[u8]) {
        if self.failure.is_some() {
            return;
        }
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_BYTES {
            self.fail("incoming H2 byte cap exceeded");
            return;
        }
        while !bytes.is_empty() {
            if self.pending.is_some() {
                let consumed = self.remaining.min(bytes.len());
                self.remaining -= consumed;
                bytes = &bytes[consumed..];
                if self.remaining == 0 {
                    self.finish_frame();
                }
                continue;
            }
            let copied = (9 - self.header_used).min(bytes.len());
            self.header[self.header_used..self.header_used + copied].copy_from_slice(&bytes[..copied]);
            self.header_used += copied;
            bytes = &bytes[copied..];
            if self.header_used != 9 {
                continue;
            }
            self.frames += 1;
            if self.frames > MAX_FRAMES {
                self.fail("incoming H2 frame cap exceeded");
                return;
            }
            self.remaining =
                (usize::from(self.header[0]) << 16) | (usize::from(self.header[1]) << 8) | usize::from(self.header[2]);
            if self.remaining > MAX_BYTES {
                self.fail("incoming H2 payload exceeds byte cap");
                return;
            }
            self.pending = Some(FrameMeta {
                kind: self.header[3],
                flags: self.header[4],
                stream: u32::from_be_bytes(self.header[5..9].try_into().unwrap()) & 0x7fff_ffff,
            });
            self.header_used = 0;
            if self.remaining == 0 {
                self.finish_frame();
            }
        }
    }

    fn finish_frame(&mut self) {
        if let Some(frame) = self.pending.take() {
            if frame.stream == 1 {
                self.stream_frames.push(frame);
            }
        }
    }

    fn eof(&mut self) {
        if self.header_used != 0 || self.pending.is_some() {
            self.fail("EOF truncated an incoming H2 frame");
        }
    }

    fn verify(&self, stream: u32) -> Result<(), String> {
        if let Some(reason) = self.failure {
            return Err(reason.into());
        }
        if self.header_used != 0 || self.pending.is_some() {
            return Err("observed exchange ended inside an incoming H2 frame".into());
        }
        if stream != 1 {
            return Err(format!("fresh connection used unexpected stream {stream}"));
        }
        let heads: Vec<_> = self.stream_frames.iter().filter(|frame| frame.kind == 1).collect();
        if heads.len() != 1 || heads[0].flags & 1 == 0 {
            return Err(format!("expected one HEADERS END_STREAM: {:?}", self.stream_frames));
        }
        if self
            .stream_frames
            .iter()
            .any(|frame| frame.kind == 0 || frame.kind == 3)
        {
            return Err(format!(
                "terminal response included DATA or RST_STREAM: {:?}",
                self.stream_frames
            ));
        }
        Ok(())
    }
}

struct ObservedStream {
    inner: TcpStream,
    scan: Arc<Mutex<FrameScan>>,
}

impl AsyncRead for ObservedStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        match &result {
            Poll::Ready(Ok(())) => {
                let mut scan = self.scan.lock();
                if buf.filled().len() == before {
                    scan.eof();
                } else {
                    scan.observe(&buf.filled()[before..]);
                }
                if let Some(reason) = scan.failure {
                    return Poll::Ready(Err(io::Error::other(reason)));
                }
            }
            Poll::Ready(Err(_)) => self.scan.lock().fail("incoming H2 read failed"),
            Poll::Pending => {}
        }
        result
    }
}

impl AsyncWrite for ObservedStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct DriverGuard(tokio::task::JoinHandle<Result<(), h2::Error>>);
impl Drop for DriverGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone, Copy)]
enum Entry {
    Request,
    Mapped,
    Response,
}
impl Entry {
    fn name(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Mapped => "mapped",
            Self::Response => "response",
        }
    }
    fn status(self) -> &'static str {
        if matches!(self, Self::Mapped) {
            "16"
        } else {
            "8"
        }
    }
    fn mode(self) -> &'static str {
        if matches!(self, Self::Response) {
            "terminal-grpc-response"
        } else {
            "terminal-grpc-request"
        }
    }
}

fn request_frame() -> Bytes {
    let payload = super::test::HelloRequest {
        name: "terminal-envelope".into(),
    }
    .encode_to_vec();
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(0);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Bytes::from(frame)
}

fn verify_headers(headers: &HeaderMap, entry: Entry, marker: &str, web: bool) -> Result<(), String> {
    let value = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let trace = crate::access_log_client::trace_id_for_marker(marker);
    for (name, expected) in [
        ("x-terminal-order", "last"),
        ("x-trace-id", trace.as_str()),
        ("x-arch-request-id", marker),
    ] {
        if value(name) != Some(expected) || headers.get_all(name).iter().count() != 1 {
            return Err(format!("unexpected {name}: {:?}", headers.get_all(name)));
        }
    }
    let cookies: Vec<_> = headers
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap_or(""))
        .collect();
    if cookies != ["arch-first=1", "arch-second=2"] || headers.contains_key("x-terminal-removed") {
        return Err(format!("queue order/drain mismatch: cookies={cookies:?}"));
    }
    let defaults = match entry {
        Entry::Request => (Some("adapter-default"), None),
        Entry::Mapped => (None, None),
        Entry::Response => (Some("Edgion"), None),
    };
    if value("server") != defaults.0 || value("strict-transport-security") != defaults.1 {
        return Err(format!("defaults/condition ordering mismatch: {headers:?}"));
    }
    let media = value("content-type").unwrap_or("");
    if media
        != if web {
            "application/grpc-web"
        } else {
            "application/grpc"
        }
        || headers.contains_key("content-length")
        || (!web && headers.contains_key("transfer-encoding"))
    {
        return Err(format!("invalid terminal framing/media: {headers:?}"));
    }
    if web {
        if headers.contains_key("grpc-status") || headers.contains_key("grpc-message") {
            return Err("gRPC-Web initial headers contain terminal status fields".into());
        }
    } else {
        verify_status(headers, entry)?;
    }
    Ok(())
}

fn verify_status(headers: &HeaderMap, entry: Entry) -> Result<(), String> {
    if headers.get("grpc-status").and_then(|v| v.to_str().ok()) != Some(entry.status())
        || headers.get_all("grpc-status").iter().count() != 1
    {
        return Err(format!("incorrect canonical status: {headers:?}"));
    }
    let message = headers.get("grpc-message").and_then(|v| v.to_str().ok());
    if matches!(entry, Entry::Mapped) {
        if message != Some("unauthenticated") || headers.get_all("grpc-message").iter().count() != 1 {
            return Err(format!("incorrect mapped message: {headers:?}"));
        }
    } else if message.is_some() {
        return Err("absent exact message inherited queued text".into());
    }
    Ok(())
}

async fn native(ctx: &TestContext, entry: Entry, marker: &str) -> Result<(), String> {
    let tcp = TcpStream::connect((ctx.target_host.as_str(), ctx.grpc_port))
        .await
        .map_err(|e| e.to_string())?;
    let scan = Arc::new(Mutex::new(FrameScan::default()));
    let io = ObservedStream {
        inner: tcp,
        scan: Arc::clone(&scan),
    };
    let (mut client, connection) = h2::client::handshake(io).await.map_err(|e| e.to_string())?;
    let _driver = DriverGuard(tokio::spawn(connection));
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "http://terminal-grpc-{}.example.com/test.TestService/SayHello",
            entry.name()
        ))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header("x-ext-proc-test-mode", entry.mode())
        .header("traceparent", crate::access_log_client::traceparent_for_marker(marker))
        .header("x-arch-request-id", marker)
        .header("access_log", "test_store")
        .body(())
        .map_err(|e| e.to_string())?;
    let (response, mut request_body) = client.send_request(request, false).map_err(|e| e.to_string())?;
    let stream = request_body.stream_id().as_u32();
    request_body
        .send_data(request_frame(), true)
        .map_err(|e| e.to_string())?;
    let response = response.await.map_err(|e| e.to_string())?;
    if response.status() != 200 {
        return Err(format!("expected native HTTP200: {}", response.status()));
    }
    verify_headers(response.headers(), entry, marker, false)?;
    let mut body = response.into_body();
    if !body.is_end_stream()
        || body.data().await.is_some()
        || body.trailers().await.map_err(|e| e.to_string())?.is_some()
    {
        return Err("native terminal response was not initial HEADERS with clean EOS".into());
    }
    let result = scan.lock().verify(stream);
    result
}

async fn web(ctx: &TestContext, entry: Entry, marker: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .http1_only()
        .no_proxy()
        .timeout(DEADLINE)
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .post(format!(
            "http://{}:{}/test.TestService/SayHello",
            ctx.target_host, ctx.grpc_port
        ))
        .header("host", format!("terminal-grpc-{}.example.com", entry.name()))
        .header("content-type", "application/grpc-web+proto")
        .header("x-ext-proc-test-mode", entry.mode())
        .header("traceparent", crate::access_log_client::traceparent_for_marker(marker))
        .header("x-arch-request-id", marker)
        .header("access_log", "test_store")
        .body(request_frame())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() != 200 || response.version() != http::Version::HTTP_11 {
        return Err(format!(
            "expected gRPC-Web HTTP/1.1 200: {} {:?}",
            response.status(),
            response.version()
        ));
    }
    verify_headers(response.headers(), entry, marker, true)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err("gRPC-Web response exceeds byte cap".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() < 5 || bytes[0] != 0x80 {
        return Err("gRPC-Web response lacks its single trailer frame".into());
    }
    let length = u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
    if length != bytes.len() - 5 {
        return Err("gRPC-Web response contains extra/truncated frames".into());
    }
    let trailer = std::str::from_utf8(&bytes[5..]).map_err(|e| e.to_string())?;
    let mut headers = HeaderMap::new();
    for line in trailer.split("\r\n").filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("malformed gRPC-Web trailer line")?;
        headers.append(
            http::HeaderName::from_bytes(name.as_bytes()).map_err(|e| e.to_string())?,
            http::HeaderValue::from_str(value.trim()).map_err(|e| e.to_string())?,
        );
    }
    verify_status(&headers, entry)
}

#[async_trait]
impl TestSuite for GrpcTerminalHeadersWireTestSuite {
    fn name(&self) -> &str {
        "gRPC terminal headers wire"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = Vec::new();
        for entry in [Entry::Request, Entry::Mapped, Entry::Response] {
            for web_protocol in [false, true] {
                let marker = format!(
                    "terminal_grpc_{}_{}",
                    entry.name(),
                    if web_protocol { "web" } else { "native" }
                );
                cases.push(TestCase::new(
                    marker.clone(),
                    "terminal envelope preserves ordered queue, identity and protocol framing",
                    move |ctx| {
                        let marker = marker.clone();
                        Box::pin(async move {
                            let start = Instant::now();
                            let result = tokio::time::timeout(DEADLINE, async {
                                if web_protocol {
                                    web(&ctx, entry, &marker).await?;
                                } else {
                                    native(&ctx, entry, &marker).await?;
                                }
                                let log = ctx
                                    .access_log_client()
                                    .get_access_log_with_retry(&marker, 10, 200)
                                    .await
                                    .map_err(|e| e.to_string())?;
                                if log.status_code() != Some(200)
                                    || log.data["request_info"]["grpc_status"].as_u64()
                                        != Some(if matches!(entry, Entry::Mapped) { 16 } else { 8 })
                                {
                                    return Err(format!("access log lost selected terminal outcomes: {}", log.data));
                                }
                                let expected = usize::from(matches!(entry, Entry::Response));
                                if log.upstream_count() != expected {
                                    return Err(format!("expected {expected} upstreams: {}", log.data));
                                }
                                Ok::<_, String>(())
                            })
                            .await;
                            match result {
                                Ok(Ok(())) => TestResult::passed(start.elapsed()),
                                Ok(Err(error)) => TestResult::failed(start.elapsed(), error),
                                Err(_) => {
                                    TestResult::failed(start.elapsed(), "terminal wire deadline exceeded".to_string())
                                }
                            }
                        })
                    },
                ));
            }
        }
        cases
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERMINAL: &[u8] = &[0, 0, 1, 1, 5, 0, 0, 0, 1, b'x'];

    #[test]
    fn terminal_scanner_accepts_every_read_boundary() {
        for split in 0..=TERMINAL.len() {
            let mut scan = FrameScan::default();
            scan.observe(&TERMINAL[..split]);
            scan.observe(&TERMINAL[split..]);
            assert!(scan.verify(1).is_ok(), "split={split}");
        }
        let mut scan = FrameScan::default();
        for byte in TERMINAL {
            scan.observe(&[*byte]);
        }
        assert!(scan.verify(1).is_ok());
    }

    #[test]
    fn terminal_scanner_caps_control_traffic_and_keeps_failures_sticky() {
        let settings = [0, 0, 0, 4, 0, 0, 0, 0, 0];
        let mut scan = FrameScan::default();
        for _ in 0..=MAX_FRAMES {
            scan.observe(&settings);
        }
        scan.observe(TERMINAL);
        assert_eq!(scan.failure, Some("incoming H2 frame cap exceeded"));
        let mut scan = FrameScan::default();
        scan.observe(&vec![0; MAX_BYTES + 1]);
        scan.observe(TERMINAL);
        assert_eq!(scan.failure, Some("incoming H2 byte cap exceeded"));
    }

    #[test]
    fn terminal_scanner_rejects_truncated_and_data_bearing_exchanges() {
        for split in 1..TERMINAL.len() {
            let mut scan = FrameScan::default();
            scan.observe(&TERMINAL[..split]);
            scan.eof();
            scan.observe(TERMINAL);
            assert!(scan.verify(1).is_err(), "split={split}");
        }
        let mut scan = FrameScan::default();
        scan.observe(TERMINAL);
        scan.observe(&[0, 0, 0, 0, 1, 0, 0, 0, 1]);
        assert!(
            scan.verify(1).is_err(),
            "even empty DATA invalidates trailers-only proof"
        );
    }
}
