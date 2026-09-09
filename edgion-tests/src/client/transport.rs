//! Transport-level client primitives for the request-body/transport suite.
//!
//! Three things live here, none of which the framework could express before:
//!
//! * [`RawH1Conn`] — a hand-driven HTTP/1.1 connection. Half-close, an abortive
//!   `SO_LINGER(0)` close, request pipelining, chunked bodies with a *real*
//!   trailer section, and reading an interim `1xx` without consuming the final
//!   response. These generalise the three ad-hoc raw helpers that already exist
//!   in `suites/edgion_plugins/{body_plugins.rs,dsl/body.rs}` — those hardcode
//!   port 31180, which is exactly why they could not be reused.
//! * [`RawH2Conn`] — a raw `h2` client. Client-initiated RST_STREAM, trailers,
//!   and a second stream on the *same* connection.
//! * [`TransportProbe`] — the assertion layer over the test server's
//!   `/transport/query/{trace_id}` recorder. Its `wait_for` / `expect_none` /
//!   `dump` API is a port of the fork harness's `Recorder`, with the same
//!   `#[must_use]` discipline, so a dropped wait is a compile warning rather
//!   than a silently vacuous test.
//!
//! **Why the probe and not echoed response headers.** Every negative claim in
//! this area ("the connection was not reused", "no body byte reached the
//! upstream", "the gateway reset the stream") is a claim about what the gateway
//! put on the wire *toward the upstream*. A response header proves only what the
//! backend application saw, and on the paths that matter no response is ever
//! written. See `edgion-tests/src/server/transport/recorder.rs`.

use bytes::Bytes;
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

/// The test server's primary HTTP port, which hosts the `/transport/*` query
/// surface on the shared axum router (the same router that serves
/// `/mirror/query/{trace_id}`).
pub const RECORDER_PORT: u16 = 30001;

/// Listener label used by the raw HTTP/1.1 transport backend (port 30030).
pub const LISTENER_H1: &str = "h1";
/// Listener label used by the raw HTTP/2 transport backend (port 30031).
pub const LISTENER_H2: &str = "h2";

pub const KIND_CONN_ACCEPTED: &str = "ConnAccepted";
pub const KIND_REQ_HEADERS: &str = "ReqHeaders";
pub const KIND_REQ_DATA: &str = "ReqData";
pub const KIND_REQ_TRAILERS: &str = "ReqTrailers";
pub const KIND_PEER_RESET: &str = "PeerReset";
pub const KIND_PEER_GOAWAY: &str = "PeerGoaway";
pub const KIND_PEER_HALF_CLOSE: &str = "PeerHalfClose";
pub const KIND_RESP_HEAD: &str = "RespHead";
pub const KIND_RESP_DONE: &str = "RespDone";
pub const KIND_CONN_CLOSED: &str = "ConnClosed";

// ===========================================================================
// HTTP/1 primitives
// ===========================================================================

/// A parsed HTTP/1 response head. The body (if any) stays on the connection.
#[derive(Clone, Debug, Default)]
pub struct H1Response {
    pub version: String,
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
}

impl H1Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_chunked(&self) -> bool {
        self.header("transfer-encoding")
            .map(|v| v.to_ascii_lowercase().contains("chunked"))
            .unwrap_or(false)
    }

    pub fn content_length(&self) -> Option<usize> {
        self.header("content-length")
            .and_then(|v| v.trim().parse::<usize>().ok())
    }

    pub fn is_interim(&self) -> bool {
        (100..200).contains(&self.status)
    }
}

impl fmt::Display for H1Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} {} {}", self.version, self.status, self.reason)?;
        for (k, v) in &self.headers {
            writeln!(f, "  {k}: {v}")?;
        }
        Ok(())
    }
}

fn parse_head(raw: &str) -> Result<H1Response, String> {
    let mut lines = raw.split("\r\n");
    let status_line = lines.next().ok_or_else(|| "empty response head".to_string())?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default().to_string();
    if !matches!(version.as_str(), "HTTP/1.0" | "HTTP/1.1") {
        return Err(format!("unsupported response version in status line: {status_line:?}"));
    }
    let status = parts
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("unparsable status line: {status_line:?}"))?;
    let reason = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("malformed response header line: {line:?}"))?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    Ok(H1Response {
        version,
        status,
        reason,
        headers,
    })
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Render one HTTP/1 request head without writing it.
///
/// Exposed for one reason: a pipelining case must put both heads in a **single**
/// write. Pingora only notices a pipelined request if its bytes were over-read
/// into the first request's buffer (`body_buf_overread`, consulted by
/// `HttpSession::reuse` in `pingora-core/src/protocols/http/v1/server.rs`), so
/// two separate writes make the observed behavior a race against the gateway's
/// read loop rather than a property of the gateway. One write removes the race.
pub fn render_h1_head(method: &str, path: &str, host: &str, headers: &[(&str, &str)]) -> String {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    head
}

/// A hand-driven HTTP/1.1 connection to the gateway.
///
/// Every method takes the port as a parameter (via [`RawH1Conn::connect`]) —
/// the existing ad-hoc helpers hardcode 31180 and are therefore unusable for
/// any other suite's listener.
pub struct RawH1Conn {
    stream: TcpStream,
    pending: Vec<u8>,
}

impl RawH1Conn {
    pub async fn connect(host: &str, port: u16) -> Result<Self, String> {
        let stream = TcpStream::connect(format!("{host}:{port}"))
            .await
            .map_err(|e| format!("TCP connect to {host}:{port} failed: {e}"))?;
        stream.set_nodelay(true).ok();
        Ok(Self {
            stream,
            pending: Vec::new(),
        })
    }

    /// Write a request head.
    ///
    /// To *pipeline*, do not call this twice: render both heads with
    /// [`render_h1_head`] and put them on the wire in one [`RawH1Conn::write_raw`].
    /// See that function for why the difference decides the outcome.
    pub async fn write_head(
        &mut self,
        method: &str,
        path: &str,
        host: &str,
        headers: &[(&str, &str)],
    ) -> Result<(), String> {
        self.write_raw(render_h1_head(method, path, host, headers).as_bytes())
            .await
    }

    pub async fn write_raw(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.stream
            .write_all(bytes)
            .await
            .map_err(|e| format!("write failed: {e}"))?;
        self.stream.flush().await.map_err(|e| format!("flush failed: {e}"))
    }

    /// Write one chunk of a `Transfer-Encoding: chunked` body.
    pub async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), String> {
        let mut framed = format!("{:X}\r\n", chunk.len()).into_bytes();
        framed.extend_from_slice(chunk);
        framed.extend_from_slice(b"\r\n");
        self.write_raw(&framed).await
    }

    /// Terminal chunk with no trailer section.
    pub async fn finish_chunked(&mut self) -> Result<(), String> {
        self.write_raw(b"0\r\n\r\n").await
    }

    /// Terminal chunk plus a **real** trailer section.
    ///
    /// A bare `0\r\n\r\n`, or a `Trailer:` declaration in the head, does not
    /// count as trailers — the recorder applies the same detection rule the
    /// fork's `request_trailer_filter` does, so this is the only shape that
    /// makes a trailer claim assertable.
    pub async fn finish_chunked_with_trailers(&mut self, trailers: &[(&str, &str)]) -> Result<(), String> {
        let mut out = String::from("0\r\n");
        for (name, value) in trailers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        self.write_raw(out.as_bytes()).await
    }

    async fn fill_head(&mut self, timeout: Duration) -> Result<usize, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pos) = find_head_end(&self.pending) {
                return Ok(pos);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "timed out after {timeout:?} waiting for a response head (buffered {} bytes: {:?})",
                    self.pending.len(),
                    String::from_utf8_lossy(&self.pending)
                ));
            }
            let mut buf = [0u8; 4096];
            match tokio::time::timeout(remaining, self.stream.read(&mut buf)).await {
                Err(_) => {
                    return Err(format!(
                        "timed out after {timeout:?} waiting for a response head (buffered {} bytes)",
                        self.pending.len()
                    ))
                }
                Ok(Ok(0)) => {
                    return Err(format!(
                        "connection closed before a complete response head (buffered {} bytes: {:?})",
                        self.pending.len(),
                        String::from_utf8_lossy(&self.pending)
                    ))
                }
                Ok(Ok(n)) => self.pending.extend_from_slice(&buf[..n]),
                Ok(Err(e)) => return Err(format!("read failed: {e}")),
            }
        }
    }

    /// Buffer whatever the peer has already sent, waiting at most `window`, and
    /// report whether the peer said anything at all.
    ///
    /// Written for the client that has to keep uploading while it watches for a
    /// response. The shapes where the origin answers *during* the upload need a
    /// client that stops writing once the answer arrives: a client that keeps
    /// pushing a body into a connection the gateway has already finished with
    /// turns its own writes into a TCP RST, and an RST discards the response
    /// bytes still sitting in the receive buffer — so the case would destroy the
    /// very evidence it is about.
    ///
    /// EOF and a reset both count as "the peer is done": the caller's next read
    /// reports either properly, and neither leaves a reason to write more.
    pub async fn read_available(&mut self, window: Duration) -> Result<bool, String> {
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(window, self.stream.read(&mut buf)).await {
            Err(_) => Ok(false),
            Ok(Ok(0)) => Ok(true),
            Ok(Ok(n)) => {
                self.pending.extend_from_slice(&buf[..n]);
                Ok(true)
            }
            Ok(Err(_)) => Ok(true),
        }
    }

    /// Read exactly one response head, leaving any body bytes buffered.
    pub async fn read_response_head(&mut self, timeout: Duration) -> Result<H1Response, String> {
        let pos = self.fill_head(timeout).await?;
        let raw = String::from_utf8_lossy(&self.pending[..pos]).into_owned();
        self.pending.drain(..pos);
        parse_head(&raw)
    }

    /// Read an interim `1xx` response if one arrives within `timeout`.
    ///
    /// If the head that arrives is final (`>= 200`) it is **not** consumed —
    /// the bytes stay buffered so the caller's next
    /// [`RawH1Conn::read_response_head`] sees it. `Ok(None)` therefore means
    /// "no interim", never "the response was swallowed".
    pub async fn read_interim(&mut self, timeout: Duration) -> Result<Option<H1Response>, String> {
        let pos = match self.fill_head(timeout).await {
            Ok(pos) => pos,
            // A timeout here is a legitimate "no interim arrived" answer.
            Err(e) if e.starts_with("timed out") => return Ok(None),
            Err(e) => return Err(e),
        };
        let raw = String::from_utf8_lossy(&self.pending[..pos]).into_owned();
        let head = parse_head(&raw)?;
        if head.is_interim() {
            self.pending.drain(..pos);
            Ok(Some(head))
        } else {
            Ok(None)
        }
    }

    /// Require that the peer sends **nothing at all** for `window`.
    ///
    /// The downstream-side twin of [`TransportProbe::expect_none`]: a bounded
    /// window over which the byte count must stay at zero. Exact in the
    /// direction that matters — a single byte fails it, so it cannot be
    /// satisfied by a response that merely arrived in an unexpected shape.
    ///
    /// A clean close counts as silence: EOF proves no further response head was
    /// written just as convincingly as an idle connection, and which of the two
    /// the gateway chooses is not the claim.
    #[must_use = "an ignored wait asserts nothing"]
    pub async fn expect_silence(&mut self, window: Duration) -> Result<(), String> {
        if !self.pending.is_empty() {
            return Err(format!(
                "expected the peer to send nothing, but {} bytes were already buffered: {:?}",
                self.pending.len(),
                String::from_utf8_lossy(&self.pending)
            ));
        }
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(window, self.stream.read(&mut buf)).await {
            // No bytes within the window, or the peer closed: both are silence.
            Err(_) | Ok(Ok(0)) => Ok(()),
            Ok(Ok(n)) => Err(format!(
                "expected the peer to send nothing for {window:?}, but it sent {n} bytes: {:?}",
                String::from_utf8_lossy(&buf[..n])
            )),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionReset => Ok(()),
            Ok(Err(e)) => Err(format!("read failed while requiring silence: {e}")),
        }
    }

    /// Require that the peer closes the connection within `window` **without**
    /// sending anything more.
    ///
    /// Stricter than [`RawH1Conn::expect_silence`] in both directions: any byte
    /// fails ("it answered when it must not have"), and so does a connection
    /// that is still open and silent when the window ends ("it neither answered
    /// nor closed"), which is what distinguishes "refused to continue" from
    /// "hung".
    #[must_use = "an ignored wait asserts nothing"]
    pub async fn expect_close_without_response(&mut self, window: Duration) -> Result<(), String> {
        if !self.pending.is_empty() {
            return Err(format!(
                "expected the peer to close without sending anything more, but {} bytes were already \
                 buffered: {:?}",
                self.pending.len(),
                String::from_utf8_lossy(&self.pending)
            ));
        }
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(window, self.stream.read(&mut buf)).await {
            Ok(Ok(0)) => Ok(()),
            Ok(Ok(n)) => Err(format!(
                "expected the peer to close without sending anything more, but it sent {n} bytes: {:?}",
                String::from_utf8_lossy(&buf[..n])
            )),
            // A reset is still "this connection is over"; only the byte stream
            // is the claim, not which of FIN or RST ended it.
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionReset => Ok(()),
            Ok(Err(e)) => Err(format!("read failed while waiting for the peer to close: {e}")),
            Err(_) => Err(format!(
                "the peer neither sent anything nor closed within {window:?} — it hung instead"
            )),
        }
    }

    /// Read the body belonging to `head`, honouring chunked / Content-Length /
    /// read-to-EOF framing in that order.
    pub async fn read_body(&mut self, head: &H1Response, timeout: Duration) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + timeout;
        if head.is_chunked() {
            return self.read_chunked_body(deadline).await;
        }
        if let Some(len) = head.content_length() {
            while self.pending.len() < len {
                if !self.read_more(deadline).await? {
                    return Err(format!(
                        "connection closed after {} of {} declared body bytes",
                        self.pending.len(),
                        len
                    ));
                }
            }
            return Ok(self.pending.drain(..len).collect());
        }
        // No framing: read to EOF.
        loop {
            if !self.read_more(deadline).await? {
                break;
            }
        }
        Ok(std::mem::take(&mut self.pending))
    }

    async fn read_chunked_body(&mut self, deadline: Instant) -> Result<Vec<u8>, String> {
        let mut body = Vec::new();
        loop {
            let line_end = loop {
                if let Some(pos) = self.pending.windows(2).position(|w| w == b"\r\n") {
                    break pos;
                }
                if !self.read_more(deadline).await? {
                    return Err("connection closed inside a chunk-size line".to_string());
                }
            };
            let size_line = String::from_utf8_lossy(&self.pending[..line_end]).into_owned();
            let size_hex = size_line.split(';').next().unwrap_or("").trim().to_string();
            let size = usize::from_str_radix(&size_hex, 16)
                .map_err(|_| format!("unparsable chunk-size line {size_line:?}"))?;
            self.pending.drain(..line_end + 2);
            if size == 0 {
                // Consume the (possibly empty) trailer section.
                loop {
                    if let Some(pos) = self.pending.windows(2).position(|w| w == b"\r\n") {
                        let line = self.pending.drain(..pos + 2).collect::<Vec<_>>();
                        if line == b"\r\n" {
                            return Ok(body);
                        }
                        continue;
                    }
                    if !self.read_more(deadline).await? {
                        return Ok(body);
                    }
                }
            }
            while self.pending.len() < size + 2 {
                if !self.read_more(deadline).await? {
                    return Err("connection closed inside a chunk body".to_string());
                }
            }
            body.extend_from_slice(&self.pending[..size]);
            self.pending.drain(..size + 2);
        }
    }

    /// Returns `Ok(false)` on a clean EOF.
    async fn read_more(&mut self, deadline: Instant) -> Result<bool, String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("timed out reading the response body".to_string());
        }
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(remaining, self.stream.read(&mut buf)).await {
            Err(_) => Err("timed out reading the response body".to_string()),
            Ok(Ok(0)) => Ok(false),
            Ok(Ok(n)) => {
                self.pending.extend_from_slice(&buf[..n]);
                Ok(true)
            }
            Ok(Err(e)) => Err(format!("body read failed: {e}")),
        }
    }

    /// Read until the peer closes, discarding everything. Used to prove that a
    /// connection was torn down rather than kept alive.
    pub async fn wait_for_close(&mut self, timeout: Duration) -> Result<bool, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.read_more(deadline).await {
                Ok(false) => return Ok(true),
                Ok(true) => continue,
                Err(e) if e.starts_with("timed out") => return Ok(false),
                Err(e) => return Err(e),
            }
        }
    }

    /// FIN on the write half only: the client is done uploading but still reads.
    pub async fn half_close(&mut self) -> Result<(), String> {
        self.stream
            .shutdown()
            .await
            .map_err(|e| format!("write-half shutdown failed: {e}"))
    }

    /// `SO_LINGER(0)` + drop: a real TCP RST, not a graceful close.
    pub fn abort(self) {
        // tokio deprecates `set_linger` because a *non-zero* timeout blocks the
        // thread on drop. A zero timeout discards the send buffer and closes
        // immediately, which is the only way to put a real RST on the wire, and
        // there is no replacement API.
        #[allow(deprecated)]
        let _ = self.stream.set_linger(Some(Duration::ZERO));
        drop(self.stream);
    }
}

// ===========================================================================
// HTTP/2 primitives
// ===========================================================================

/// A raw `h2` client connection to the gateway's h2c-enabled plain listener.
pub struct RawH2Conn {
    sender: h2::client::SendRequest<Bytes>,
    task: JoinHandle<()>,
}

impl Drop for RawH2Conn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RawH2Conn {
    pub async fn connect(host: &str, port: u16) -> Result<Self, String> {
        let stream = TcpStream::connect(format!("{host}:{port}"))
            .await
            .map_err(|e| format!("TCP connect to {host}:{port} failed: {e}"))?;
        stream.set_nodelay(true).ok();
        let (sender, connection) = h2::client::handshake(stream)
            .await
            .map_err(|e| format!("HTTP/2 handshake failed: {e}"))?;
        let task = tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(Self { sender, task })
    }

    /// Wait until the connection can accept another stream. Required before
    /// opening a *second* stream on the same connection.
    ///
    /// `SendRequest::ready` consumes the handle; `SendRequest` is `Clone` and
    /// cloning is the documented way to keep one, so the clone is swapped back
    /// in rather than moved out of `&mut self`.
    pub async fn ready(&mut self) -> Result<(), String> {
        let sender = self.sender.clone();
        self.sender = sender
            .ready()
            .await
            .map_err(|e| format!("HTTP/2 connection is not ready for another stream: {e}"))?;
        Ok(())
    }

    pub fn send_request(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        end_stream: bool,
    ) -> Result<(h2::client::ResponseFuture, h2::SendStream<Bytes>), String> {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .version(http::Version::HTTP_2);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.body(()).map_err(|e| format!("build HTTP/2 request: {e}"))?;
        self.sender
            .send_request(request, end_stream)
            .map_err(|e| format!("send HTTP/2 request headers: {e}"))
    }

    pub fn shutdown(self) {
        self.task.abort();
    }
}

/// Send a real HTTP/2 trailer section on a request stream.
pub fn h2_send_trailers(body: &mut h2::SendStream<Bytes>, trailers: http::HeaderMap) -> Result<(), String> {
    body.send_trailers(trailers)
        .map_err(|e| format!("send HTTP/2 trailers: {e}"))
}

/// Client-initiated RST_STREAM — "the client cancels mid-upload".
pub fn h2_reset(body: &mut h2::SendStream<Bytes>, reason: h2::Reason) {
    body.send_reset(reason);
}

/// Wait for the peer's RST_STREAM on a request stream.
pub async fn h2_await_reset(body: &mut h2::SendStream<Bytes>, timeout: Duration) -> Result<h2::Reason, String> {
    tokio::time::timeout(timeout, std::future::poll_fn(|cx| body.poll_reset(cx)))
        .await
        .map_err(|_| format!("no RST_STREAM within {timeout:?}"))?
        .map_err(|e| format!("poll_reset failed: {e}"))
}

// ===========================================================================
// The recorder client
// ===========================================================================

/// One transport fact as recorded by the test server. Mirrors
/// `edgion-tests/src/server/transport/recorder.rs::TransportEvent`; it is
/// redeclared here because that module is re-rooted into the `test_server`
/// **binary**, not the shared library.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct TransportEvent {
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub at_ms: u64,
    #[serde(default)]
    pub listener: String,
    #[serde(default)]
    pub conn: u32,
    #[serde(default)]
    pub stream: u32,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub trace_request_ordinal: Option<u32>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub headers_eos: Option<bool>,
    #[serde(default)]
    pub content_length: Option<u64>,
    #[serde(default)]
    pub transfer_encoding: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub grpc_timeout_value: Option<u64>,
    #[serde(default)]
    pub grpc_timeout_unit: Option<String>,
    #[serde(default)]
    pub len: Option<usize>,
    #[serde(default)]
    pub end_stream: Option<bool>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub behavior: Option<String>,
    #[serde(default)]
    pub framing: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

impl TransportEvent {
    pub fn is(&self, kind: &str) -> bool {
        self.kind == kind
    }

    pub fn tagged(&self, trace_id: &str) -> bool {
        self.trace_id.as_deref() == Some(trace_id)
    }
}

impl fmt::Display for TransportEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "seq={:<3} at_ms={:<6} {}/conn{}/s{} {:<14}",
            self.seq, self.at_ms, self.listener, self.conn, self.stream, self.kind
        )?;
        if let Some(t) = &self.trace_id {
            write!(f, " trace={t}")?;
        }
        if let Some(v) = self.trace_request_ordinal {
            write!(f, " trace_request={v}")?;
        }
        if let Some(v) = self.headers_eos {
            write!(f, " headers_eos={v}")?;
        }
        if let Some(v) = self.content_length {
            write!(f, " cl={v}")?;
        }
        if let Some(v) = &self.transfer_encoding {
            write!(f, " te={v}")?;
        }
        if let Some(v) = &self.method {
            write!(f, " {v}")?;
        }
        if let Some(v) = &self.path {
            write!(f, " {v}")?;
        }
        if let (Some(value), Some(unit)) = (self.grpc_timeout_value, &self.grpc_timeout_unit) {
            write!(f, " grpc_timeout={value}{unit}")?;
        }
        if let Some(v) = self.len {
            write!(f, " len={v}")?;
        }
        if let Some(v) = self.end_stream {
            write!(f, " eos={v}")?;
        }
        if let Some(v) = self.status {
            write!(f, " status={v}")?;
        }
        if let Some(v) = &self.code {
            write!(f, " code={v}")?;
        }
        if let Some(v) = &self.behavior {
            write!(f, " behavior={v}")?;
        }
        if let Some(v) = &self.framing {
            write!(f, " framing={v}")?;
        }
        if let Some(v) = &self.detail {
            write!(f, " detail={v}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct ConnSummary {
    #[serde(default)]
    pub listener: String,
    #[serde(default)]
    pub conn: u32,
    #[serde(default)]
    pub requests: u32,
    #[serde(default)]
    pub traces: Vec<String>,
    #[serde(default)]
    pub closed: bool,
}

/// One `/transport/query/{trace_id}` answer.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct TransportSnapshot {
    #[serde(default)]
    pub found: bool,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    events: Vec<TransportEvent>,
    #[serde(default)]
    connections: Vec<ConnSummary>,
    #[serde(default)]
    conn_accepted_total: HashMap<String, u64>,
    #[serde(default)]
    pub dropped: u64,
}

impl TransportSnapshot {
    pub fn events(&self) -> &[TransportEvent] {
        &self.events
    }

    pub fn connections(&self) -> &[ConnSummary] {
        &self.connections
    }

    pub fn conn_accepted_total(&self, listener: &str) -> u64 {
        self.conn_accepted_total.get(listener).copied().unwrap_or(0)
    }

    pub fn find(&self, pred: impl Fn(&TransportEvent) -> bool) -> Option<&TransportEvent> {
        self.events.iter().find(|e| pred(e))
    }

    /// Exact count over the **whole** log. Anti-vacuity: a claim like "no second
    /// connection was opened" is asserted as `count_matching(..) == 1`, never as
    /// `<= 1`, which an empty log also satisfies.
    pub fn count_matching(&self, pred: impl Fn(&TransportEvent) -> bool) -> usize {
        self.events.iter().filter(|e| pred(e)).count()
    }

    /// `at_ms` of the first matching event — the anchor an ordering claim needs
    /// in order to be *able* to fail.
    pub fn first_seen(&self, pred: impl Fn(&TransportEvent) -> bool) -> Option<u64> {
        self.events.iter().find(|e| pred(e)).map(|e| e.at_ms)
    }

    /// Total request-body bytes the upstream recorded for `trace_id`.
    pub fn req_body_total(&self, trace_id: &str) -> usize {
        self.events
            .iter()
            .filter(|e| e.is(KIND_REQ_DATA) && e.tagged(trace_id))
            .filter_map(|e| e.len)
            .sum()
    }

    /// Every connection ordinal that carried `trace_id`.
    pub fn conns_for(&self, trace_id: &str) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = self
            .events
            .iter()
            .filter(|e| e.tagged(trace_id))
            .map(|e| (e.listener.clone(), e.conn))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn foreclosed(&self) -> bool {
        !self.connections.is_empty() && self.connections.iter().all(|c| c.closed)
    }
}

impl fmt::Display for TransportSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "--- transport log: found={} count={} dropped={} accepted_total={:?}",
            self.found, self.count, self.dropped, self.conn_accepted_total
        )?;
        for c in &self.connections {
            writeln!(
                f,
                "    conn {}/{}: requests={} closed={} traces={:?}",
                c.listener, c.conn, c.requests, c.closed, c.traces
            )?;
        }
        for e in &self.events {
            writeln!(f, "    {e}")?;
        }
        write!(f, "--- end transport log")
    }
}

/// The assertion layer over `/transport/query/{trace_id}`.
pub struct TransportProbe {
    base: String,
    trace_id: String,
    client: reqwest::Client,
}

impl TransportProbe {
    /// Reset the recorder and mint a fresh trace id. Every case starts here.
    pub async fn start(backend_host: &str, backend_http_port: u16, label: &str) -> Result<Self, String> {
        let base = format!("http://{backend_host}:{backend_http_port}");
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("build probe HTTP client: {e}"))?;
        let response = client
            .get(format!("{base}/transport/reset"))
            .send()
            .await
            .map_err(|e| format!("/transport/reset failed (is test_server running with --transport-h1-port/--transport-h2-port?): {e}"))?;
        if !response.status().is_success() {
            return Err(format!("/transport/reset returned {}", response.status()));
        }
        let trace_id = format!("{label}-{}", uuid::Uuid::new_v4());
        Ok(Self { base, trace_id, client })
    }

    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    /// Mint an additional trace id sharing this probe's recorder session, for
    /// cases that drive two requests and need to tell them apart.
    pub fn sibling_trace(&self, label: &str) -> String {
        format!("{}-{label}", self.trace_id)
    }

    pub async fn snapshot(&self) -> Result<TransportSnapshot, String> {
        self.snapshot_of(&self.trace_id).await
    }

    pub async fn snapshot_of(&self, trace_id: &str) -> Result<TransportSnapshot, String> {
        let response = self
            .client
            .get(format!("{}/transport/query/{trace_id}", self.base))
            .send()
            .await
            .map_err(|e| format!("/transport/query/{trace_id} failed: {e}"))?;
        response
            .json::<TransportSnapshot>()
            .await
            .map_err(|e| format!("/transport/query/{trace_id} returned unparsable JSON: {e}"))
    }

    /// Poll until an event matching `pred` appears for this probe's trace.
    ///
    /// Fast-fails when every connection carrying the trace has closed without
    /// the event, rather than burning the whole timeout. The error text carries
    /// the full event dump.
    #[must_use = "an ignored wait asserts nothing"]
    pub async fn wait_for(
        &self,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&TransportEvent) -> bool,
    ) -> Result<TransportEvent, String> {
        self.wait_for_trace(&self.trace_id, what, timeout, pred).await
    }

    #[must_use = "an ignored wait asserts nothing"]
    pub async fn wait_for_trace(
        &self,
        trace_id: &str,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&TransportEvent) -> bool,
    ) -> Result<TransportEvent, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = self.snapshot_of(trace_id).await?;
            if let Some(found) = snapshot.find(&pred) {
                return Ok(found.clone());
            }
            if snapshot.foreclosed() {
                return Err(format!(
                    "expected {what} for trace {trace_id}, but every connection that carried it has closed without it\n{snapshot}"
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "expected {what} for trace {trace_id} within {timeout:?}, never observed\n{snapshot}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Require that NO event matching `pred` is recorded for `window`, counted
    /// **from this call**.
    ///
    /// Replaces every `count <= N` form, which an unmoved counter also
    /// satisfies. Pair it with an exact whole-log count
    /// ([`TransportSnapshot::count_matching`]) so the claim covers the period
    /// before the call as well.
    #[must_use = "an ignored wait asserts nothing"]
    pub async fn expect_none(
        &self,
        what: &str,
        window: Duration,
        pred: impl Fn(&TransportEvent) -> bool,
    ) -> Result<(), String> {
        self.expect_none_for(&self.trace_id, what, window, pred).await
    }

    /// [`TransportProbe::expect_none`] scoped to a trace other than this
    /// probe's own.
    ///
    /// Not a convenience. `/transport/query/{trace}` answers with *that*
    /// trace's events plus untagged events on its carriers, so an `expect_none`
    /// written against a sibling trace's tag while querying the primary trace
    /// forbids an event the answer can never contain — it would pass however
    /// badly the gateway misbehaved. A negative claim about a sibling trace
    /// must be asked of that sibling trace.
    #[must_use = "an ignored wait asserts nothing"]
    pub async fn expect_none_for(
        &self,
        trace_id: &str,
        what: &str,
        window: Duration,
        pred: impl Fn(&TransportEvent) -> bool,
    ) -> Result<(), String> {
        // Sequence numbers are global to the recorder, so a cursor taken from a
        // trace-scoped answer is never *later* than the true one. When the
        // scoped answer is empty the cursor is 0 and the window becomes the
        // whole log, which is stronger than asked for, never weaker.
        let cursor = self
            .snapshot_of(trace_id)
            .await?
            .events()
            .iter()
            .map(|e| e.seq)
            .max()
            .unwrap_or(0);
        let deadline = Instant::now() + window;
        loop {
            let snapshot = self.snapshot_of(trace_id).await?;
            if let Some(found) = snapshot.find(|e| e.seq > cursor && pred(e)) {
                return Err(format!(
                    "expected no {what} after seq {cursor}, but observed: {found}\n{snapshot}"
                ));
            }
            if Instant::now() >= deadline {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The whole recorder log, for a failure message.
    pub async fn dump(&self) -> String {
        match self.client.get(format!("{}/transport/dump", self.base)).send().await {
            Ok(response) => response
                .text()
                .await
                .unwrap_or_else(|e| format!("<dump body unreadable: {e}>")),
            Err(e) => format!("<dump unavailable: {e}>"),
        }
    }
}

/// Render a probe failure as text rather than an escaped `Debug` string.
pub fn probe_err(what: &str, e: String) -> String {
    format!("{what}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(seq: u64, kind: &str, trace: Option<&str>) -> TransportEvent {
        TransportEvent {
            seq,
            kind: kind.to_string(),
            trace_id: trace.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn snapshot_json_round_trips_the_recorder_shape() {
        let raw = r#"{
          "found": true,
          "count": 3,
          "events": [
            {"seq":1,"at_ms":4,"listener":"h1","conn":0,"stream":0,"kind":"ConnAccepted"},
            {"seq":2,"at_ms":5,"listener":"h1","conn":0,"stream":1,"trace_id":"t1",
             "kind":"ReqHeaders","headers_eos":false,"content_length":4,"method":"POST","path":"/x"},
            {"seq":3,"at_ms":5,"listener":"h1","conn":0,"stream":1,"trace_id":"t1",
             "kind":"ReqData","len":4,"end_stream":true}
          ],
          "connections": [{"listener":"h1","conn":0,"requests":1,"traces":["t1"],"closed":false}],
          "conn_accepted_total": {"h1": 1},
          "dropped": 0
        }"#;
        let snapshot: TransportSnapshot = serde_json::from_str(raw).expect("snapshot must deserialize");
        assert!(snapshot.found);
        assert_eq!(snapshot.count, 3);
        assert_eq!(snapshot.events().len(), 3);
        assert_eq!(snapshot.conn_accepted_total("h1"), 1);
        assert_eq!(snapshot.conn_accepted_total("h2"), 0);
        assert_eq!(snapshot.req_body_total("t1"), 4);
        assert_eq!(snapshot.conns_for("t1"), vec![("h1".to_string(), 0)]);
        assert_eq!(snapshot.count_matching(|e| e.is(KIND_REQ_DATA)), 1);
        assert_eq!(snapshot.first_seen(|e| e.is(KIND_REQ_DATA)), Some(5));
        assert!(!snapshot.foreclosed());
    }

    #[test]
    fn a_fully_closed_connection_set_forecloses_a_wait() {
        let raw = r#"{"found":true,"count":1,
          "events":[{"seq":1,"at_ms":0,"listener":"h1","conn":0,"stream":0,"kind":"ConnClosed"}],
          "connections":[{"listener":"h1","conn":0,"requests":1,"traces":["t1"],"closed":true}],
          "conn_accepted_total":{"h1":1},"dropped":0}"#;
        let snapshot: TransportSnapshot = serde_json::from_str(raw).unwrap();
        assert!(snapshot.foreclosed(), "a closed carrier must foreclose the wait");
    }

    #[test]
    fn an_empty_log_never_forecloses() {
        let snapshot = TransportSnapshot::default();
        assert!(
            !snapshot.foreclosed(),
            "an empty log must keep waiting, not report the event will never come"
        );
    }

    #[test]
    fn expect_none_cursor_ignores_events_recorded_before_the_call() {
        // `expect_none` filters on `seq > cursor`; this pins that rule so a
        // refactor cannot turn the window into "the whole log".
        let events = vec![ev(1, KIND_CONN_ACCEPTED, None), ev(2, KIND_REQ_HEADERS, Some("t1"))];
        let cursor = events.iter().map(|e| e.seq).max().unwrap_or(0);
        assert_eq!(cursor, 2);
        assert!(!events.iter().any(|e| e.seq > cursor && e.is(KIND_CONN_ACCEPTED)));
        let mut later = events.clone();
        later.push(ev(3, KIND_CONN_ACCEPTED, None));
        assert!(later.iter().any(|e| e.seq > cursor && e.is(KIND_CONN_ACCEPTED)));
    }

    #[test]
    fn head_parsing_keeps_header_order_and_finds_the_head_boundary() {
        let raw = "HTTP/1.1 100 Continue\r\n\r\n";
        assert_eq!(find_head_end(raw.as_bytes()), Some(raw.len()));
        let head = parse_head("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Up-Conn: 3\r\n").unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.content_length(), Some(5));
        assert_eq!(head.header("x-up-conn"), Some("3"));
        assert!(!head.is_interim());
        assert!(parse_head("HTTP/1.1 100 Continue\r\n").unwrap().is_interim());
    }

    #[test]
    fn head_parsing_rejects_invalid_versions_and_header_lines() {
        assert!(parse_head("HTTP/2 200 OK\r\nContent-Length: 0\r\n").is_err());
        assert!(parse_head("HTTX/1.1 200 OK\r\nContent-Length: 0\r\n").is_err());
        assert!(parse_head("HTTP/1.1 200 OK\r\nHTTP/1.1 500 Duplicate\r\n").is_err());
        assert!(parse_head("HTTP/1.1 200 OK\r\nX-Good: yes\r\nbroken\r\n").is_err());
    }
}
