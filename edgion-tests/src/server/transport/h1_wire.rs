// HTTP/1.1 wire helpers for the raw transport backend.
//
// Everything here is either pure or generic over `AsyncBufRead`, so it is unit
// testable against an in-memory cursor with no listener, no gateway and no
// integration stack. The socket-level connection loop lives in `h1.rs`.

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

use super::script::Malformed;

/// Largest request head this backend accepts.
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Largest single request-body read.
const MAX_BODY_READ: usize = 8 * 1024;
/// Largest single chunk this backend accepts.
const MAX_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// A parsed HTTP/1 request head. Header names are lowercased; values keep their
/// original spelling, because a framing assertion is about the exact value the
/// gateway wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    /// `"1.0"` / `"1.1"`, taken verbatim from the request line.
    pub version: String,
    pub headers: Vec<(String, String)>,
}

impl RequestHead {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn content_length(&self) -> Option<u64> {
        self.get("content-length").and_then(|v| v.trim().parse().ok())
    }

    pub fn transfer_encoding(&self) -> Option<&str> {
        self.get("transfer-encoding")
    }

    pub fn is_chunked(&self) -> bool {
        self.transfer_encoding()
            .map(|v| v.to_ascii_lowercase().contains("chunked"))
            .unwrap_or(false)
    }

    /// The H1 analogue of "HEADERS carried END_STREAM": the head declares no
    /// body at all — neither `Content-Length` nor `Transfer-Encoding`.
    pub fn headers_eos(&self) -> bool {
        self.content_length().is_none() && self.transfer_encoding().is_none()
    }

    pub fn framing(&self) -> BodyFraming {
        if self.is_chunked() {
            BodyFraming::Chunked
        } else {
            match self.content_length() {
                Some(n) if n > 0 => BodyFraming::ContentLength(n),
                _ => BodyFraming::None,
            }
        }
    }
}

/// How the request body is framed on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyFraming {
    None,
    ContentLength(u64),
    Chunked,
}

/// Parse a complete request head (request line + fields, no trailing body).
pub fn parse_request_head(raw: &str) -> Option<RequestHead> {
    let mut lines = raw.split("\r\n").flat_map(|l| l.split('\n'));
    let request_line = lines.next()?.trim_end();
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let version = parts
        .next()
        .and_then(|v| v.strip_prefix("HTTP/"))
        .unwrap_or("1.1")
        .to_string();

    let mut headers = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    Some(RequestHead {
        method,
        path,
        version,
        headers,
    })
}

/// Read one request head off the wire.
///
/// `Ok(None)` means the peer closed cleanly at a message boundary — for an
/// upstream backend, that is the gateway declining to reuse the connection, and
/// the caller records it as `PeerHalfClose`.
pub async fn read_head<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<String>, String> {
    let mut raw = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = reader
            .read_until(b'\n', &mut line)
            .await
            .map_err(|e| format!("read head: {e}"))?;
        if n == 0 {
            return if raw.is_empty() {
                Ok(None)
            } else {
                Err("connection closed inside a request head".to_string())
            };
        }
        let terminator = line == b"\r\n" || line == b"\n";
        raw.extend_from_slice(&line);
        if raw.len() > MAX_HEAD_BYTES {
            return Err(format!("request head exceeded {MAX_HEAD_BYTES} bytes"));
        }
        if terminator {
            break;
        }
    }
    String::from_utf8(raw)
        .map(Some)
        .map_err(|e| format!("request head is not UTF-8: {e}"))
}

/// One step of request-body consumption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyItem {
    Data(Vec<u8>),
    /// Real trailer fields in the chunked terminal section. A bare `0\r\n\r\n`
    /// yields `End` instead — this mirrors the fork's detection rule, where a
    /// `Trailer:` declaration alone does not count as trailers.
    Trailers(Vec<(String, String)>),
    /// The request body ended normally.
    End,
    /// The peer closed (or reset) before the body was complete.
    PeerClosed,
}

/// Incremental request-body reader for both H1 framings.
#[derive(Debug)]
pub struct BodyPump {
    framing: BodyFraming,
    remaining: u64,
    finished: bool,
    trailers_emitted: bool,
}

impl BodyPump {
    pub fn new(framing: BodyFraming) -> Self {
        let remaining = match framing {
            BodyFraming::ContentLength(n) => n,
            _ => 0,
        };
        Self {
            framing,
            remaining,
            finished: matches!(framing, BodyFraming::None),
            trailers_emitted: false,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub async fn next<R: AsyncBufRead + Unpin>(&mut self, reader: &mut R) -> Result<BodyItem, String> {
        if self.finished {
            return Ok(BodyItem::End);
        }
        match self.framing {
            BodyFraming::None => {
                self.finished = true;
                Ok(BodyItem::End)
            }
            BodyFraming::ContentLength(_) => self.next_content_length(reader).await,
            BodyFraming::Chunked => self.next_chunked(reader).await,
        }
    }

    async fn next_content_length<R: AsyncBufRead + Unpin>(&mut self, reader: &mut R) -> Result<BodyItem, String> {
        let take = std::cmp::min(self.remaining, MAX_BODY_READ as u64) as usize;
        let mut buf = vec![0u8; take];
        let n = reader.read(&mut buf).await.map_err(|e| format!("read body: {e}"))?;
        if n == 0 {
            self.finished = true;
            return Ok(BodyItem::PeerClosed);
        }
        buf.truncate(n);
        self.remaining -= n as u64;
        if self.remaining == 0 {
            self.finished = true;
        }
        Ok(BodyItem::Data(buf))
    }

    async fn next_chunked<R: AsyncBufRead + Unpin>(&mut self, reader: &mut R) -> Result<BodyItem, String> {
        if self.trailers_emitted {
            self.finished = true;
            return Ok(BodyItem::End);
        }

        let Some(size_line) = read_line(reader).await? else {
            self.finished = true;
            return Ok(BodyItem::PeerClosed);
        };
        let size_token = size_line.split(';').next().unwrap_or("").trim();
        let size =
            u64::from_str_radix(size_token, 16).map_err(|_| format!("invalid chunk size line {:?}", size_line))?;
        if size > MAX_CHUNK_BYTES {
            return Err(format!("chunk size {size} exceeds {MAX_CHUNK_BYTES}"));
        }

        if size == 0 {
            let fields = read_trailer_section(reader).await?;
            if fields.is_empty() {
                self.finished = true;
                Ok(BodyItem::End)
            } else {
                self.trailers_emitted = true;
                Ok(BodyItem::Trailers(fields))
            }
        } else {
            let mut buf = vec![0u8; size as usize];
            reader
                .read_exact(&mut buf)
                .await
                .map_err(|e| format!("read chunk body: {e}"))?;
            // Consume the chunk's trailing CRLF.
            let _ = read_line(reader).await?;
            Ok(BodyItem::Data(buf))
        }
    }
}

async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<String>, String> {
    let mut line = Vec::new();
    let n = reader
        .read_until(b'\n', &mut line)
        .await
        .map_err(|e| format!("read line: {e}"))?;
    if n == 0 {
        return Ok(None);
    }
    while line.last() == Some(&b'\n') || line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|e| format!("line is not UTF-8: {e}"))
}

async fn read_trailer_section<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Vec<(String, String)>, String> {
    let mut fields = Vec::new();
    loop {
        match read_line(reader).await? {
            None => return Ok(fields),
            Some(line) if line.is_empty() => return Ok(fields),
            Some(line) => {
                if let Some((name, value)) = line.split_once(':') {
                    fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
                }
            }
        }
    }
}

/// A response the backend is about to put on the wire, plus the facts the
/// recorder needs about what it actually emitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedResponse {
    pub bytes: Vec<u8>,
    pub status: u16,
    /// Human-readable description of the framing actually written. Recorded on
    /// the `RespHead` event, so a malformed-framing case asserts what went on
    /// the wire rather than what it hoped went on the wire.
    pub framing: String,
    /// Body bytes actually written (not what the headers claim).
    pub body_len: usize,
    /// True when the response is deliberately unterminated, so the caller must
    /// close the connection instead of trying to serve another request.
    pub must_close: bool,
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

fn status_line(status: u16) -> String {
    format!("HTTP/1.1 {} {}\r\n", status, reason_phrase(status))
}

fn push_headers(out: &mut Vec<u8>, headers: &[(String, String)]) {
    for (name, value) in headers {
        out.extend_from_slice(format!("{}: {}\r\n", name, value).as_bytes());
    }
}

/// A well-formed `Content-Length` framed response.
pub fn render_response(status: u16, headers: &[(String, String)], body: &[u8], keep_alive: bool) -> RenderedResponse {
    let mut out = Vec::new();
    out.extend_from_slice(status_line(status).as_bytes());
    push_headers(&mut out, headers);
    out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(if keep_alive {
        b"connection: keep-alive\r\n" as &[u8]
    } else {
        b"connection: close\r\n" as &[u8]
    });
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    RenderedResponse {
        bytes: out,
        status,
        framing: format!("content-length={}", body.len()),
        body_len: body.len(),
        must_close: !keep_alive,
    }
}

/// A deliberately malformed response.
///
/// This exists because the change under test touches request framing and a
/// smuggling-adjacent defect was found during its review: the gateway must be
/// observed refusing (or at least not propagating) framing that disagrees with
/// itself, and no well-behaved backend can produce that input.
///
/// Every variant closes the connection afterwards: a response whose framing is
/// a lie leaves the connection un-delimited by construction.
pub fn render_malformed(kind: Malformed, status: u16, headers: &[(String, String)], body: &[u8]) -> RenderedResponse {
    let mut out = Vec::new();
    out.extend_from_slice(status_line(status).as_bytes());
    push_headers(&mut out, headers);

    let framing = match kind {
        Malformed::ContentLengthAndChunked => {
            // Both framings declared, body written as chunked. A CL-preferring
            // parser and a TE-preferring parser disagree about where this
            // message ends — the canonical desync primitive.
            out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
            out.extend_from_slice(b"transfer-encoding: chunked\r\n\r\n");
            out.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\r\n0\r\n\r\n");
            format!("content-length={}+chunked", body.len())
        }
        Malformed::ChunkSizeOverstated => {
            let claimed = body.len() + 8;
            out.extend_from_slice(b"transfer-encoding: chunked\r\n\r\n");
            out.extend_from_slice(format!("{:x}\r\n", claimed).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\r\n0\r\n\r\n");
            format!("chunked;size-line={};actual={}", claimed, body.len())
        }
        Malformed::ChunkSizeUnderstated => {
            let claimed = std::cmp::max(1, body.len() / 2);
            out.extend_from_slice(b"transfer-encoding: chunked\r\n\r\n");
            out.extend_from_slice(format!("{:x}\r\n", claimed).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\r\n0\r\n\r\n");
            format!("chunked;size-line={};actual={}", claimed, body.len())
        }
        Malformed::ChunkTerminatorMissing => {
            out.extend_from_slice(b"transfer-encoding: chunked\r\n\r\n");
            out.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\r\n");
            format!("chunked;terminator=missing;actual={}", body.len())
        }
        Malformed::ChunkTerminatorCorrupt => {
            out.extend_from_slice(b"transfer-encoding: chunked\r\n\r\n");
            out.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\r\nZZ\r\n\r\n");
            format!("chunked;terminator=corrupt;actual={}", body.len())
        }
        Malformed::ContentLengthOverstated => {
            let claimed = body.len() + 16;
            out.extend_from_slice(format!("content-length: {}\r\n\r\n", claimed).as_bytes());
            out.extend_from_slice(body);
            format!("content-length={};actual={}", claimed, body.len())
        }
        Malformed::ContentLengthUnderstated => {
            let claimed = body.len() / 2;
            out.extend_from_slice(format!("content-length: {}\r\n\r\n", claimed).as_bytes());
            out.extend_from_slice(body);
            format!("content-length={};actual={}", claimed, body.len())
        }
    };

    RenderedResponse {
        bytes: out,
        status,
        framing,
        body_len: body.len(),
        must_close: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::BufReader;

    fn reader(raw: &str) -> BufReader<Cursor<Vec<u8>>> {
        BufReader::new(Cursor::new(raw.as_bytes().to_vec()))
    }

    fn text(r: &RenderedResponse) -> String {
        String::from_utf8_lossy(&r.bytes).into_owned()
    }

    // --- head parsing -----------------------------------------------------

    #[test]
    fn head_parsing_lowercases_names_and_keeps_values() {
        let head =
            parse_request_head("POST /x HTTP/1.1\r\nHost: h\r\nContent-Length: 4\r\nX-Trace-Id: T1\r\n\r\n").unwrap();
        assert_eq!(head.method, "POST");
        assert_eq!(head.path, "/x");
        assert_eq!(head.version, "1.1");
        assert_eq!(head.get("x-trace-id"), Some("T1"));
        assert_eq!(head.content_length(), Some(4));
        assert_eq!(head.framing(), BodyFraming::ContentLength(4));
        assert!(!head.headers_eos());
    }

    #[test]
    fn a_head_with_no_framing_headers_is_headers_eos() {
        let head = parse_request_head("GET /x HTTP/1.1\r\nHost: h\r\n\r\n").unwrap();
        assert!(head.headers_eos());
        assert_eq!(head.framing(), BodyFraming::None);
    }

    #[test]
    fn chunked_is_detected_inside_a_coding_list() {
        let head = parse_request_head("POST /x HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n").unwrap();
        assert!(head.is_chunked());
        assert_eq!(head.framing(), BodyFraming::Chunked);
        assert_eq!(head.transfer_encoding(), Some("gzip, chunked"));
        assert!(!head.headers_eos());
    }

    #[test]
    fn content_length_zero_is_not_a_body() {
        let head = parse_request_head("POST /x HTTP/1.1\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!(head.framing(), BodyFraming::None);
        // Still not headers-EOS: the gateway did declare a framing.
        assert!(!head.headers_eos());
    }

    // --- reading ----------------------------------------------------------

    #[tokio::test]
    async fn read_head_stops_at_the_blank_line() {
        let mut r = reader("GET / HTTP/1.1\r\nHost: h\r\n\r\nLEFTOVER");
        let head = read_head(&mut r).await.unwrap().unwrap();
        assert_eq!(head, "GET / HTTP/1.1\r\nHost: h\r\n\r\n");
        let mut rest = String::new();
        AsyncReadExt::read_to_string(&mut r, &mut rest).await.unwrap();
        assert_eq!(rest, "LEFTOVER");
    }

    #[tokio::test]
    async fn read_head_reports_a_clean_close_at_a_message_boundary() {
        let mut r = reader("");
        assert_eq!(read_head(&mut r).await.unwrap(), None);
    }

    #[tokio::test]
    async fn content_length_body_ends_with_exactly_one_end() {
        let mut r = reader("abcd");
        let mut pump = BodyPump::new(BodyFraming::ContentLength(4));
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::Data(b"abcd".to_vec()));
        assert!(pump.is_finished());
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::End);
    }

    #[tokio::test]
    async fn a_short_content_length_body_reports_peer_closed() {
        let mut r = reader("ab");
        let mut pump = BodyPump::new(BodyFraming::ContentLength(4));
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::Data(b"ab".to_vec()));
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::PeerClosed);
    }

    #[tokio::test]
    async fn chunked_body_yields_each_chunk_then_end() {
        let mut r = reader("3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n");
        let mut pump = BodyPump::new(BodyFraming::Chunked);
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::Data(b"abc".to_vec()));
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::Data(b"de".to_vec()));
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::End);
        assert!(pump.is_finished());
    }

    #[tokio::test]
    async fn a_bare_terminal_chunk_is_not_trailers() {
        let mut r = reader("0\r\n\r\n");
        let mut pump = BodyPump::new(BodyFraming::Chunked);
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::End);
    }

    #[tokio::test]
    async fn real_trailer_fields_are_reported_then_end() {
        let mut r = reader("3\r\nabc\r\n0\r\nx-checksum: 7\r\n\r\n");
        let mut pump = BodyPump::new(BodyFraming::Chunked);
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::Data(b"abc".to_vec()));
        assert_eq!(
            pump.next(&mut r).await.unwrap(),
            BodyItem::Trailers(vec![("x-checksum".to_string(), "7".to_string())])
        );
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::End);
    }

    #[tokio::test]
    async fn a_chunked_body_without_its_terminator_reports_peer_closed() {
        let mut r = reader("3\r\nabc\r\n");
        let mut pump = BodyPump::new(BodyFraming::Chunked);
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::Data(b"abc".to_vec()));
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::PeerClosed);
    }

    #[tokio::test]
    async fn chunk_extensions_are_ignored() {
        let mut r = reader("3;name=value\r\nabc\r\n0\r\n\r\n");
        let mut pump = BodyPump::new(BodyFraming::Chunked);
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::Data(b"abc".to_vec()));
        assert_eq!(pump.next(&mut r).await.unwrap(), BodyItem::End);
    }

    // --- response rendering ----------------------------------------------

    #[test]
    fn a_well_formed_response_declares_exactly_one_framing() {
        let r = render_response(200, &[("x-up-conn".to_string(), "0".to_string())], b"abcd", true);
        let s = text(&r);
        assert!(s.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(s.contains("x-up-conn: 0\r\n"));
        assert!(s.contains("content-length: 4\r\n"));
        assert!(!s.to_ascii_lowercase().contains("transfer-encoding"));
        assert!(s.ends_with("\r\n\r\nabcd"));
        assert_eq!(r.framing, "content-length=4");
        assert_eq!(r.body_len, 4);
        assert!(!r.must_close);
    }

    #[test]
    fn cl_and_te_emits_both_framings_and_a_chunked_body() {
        let r = render_malformed(Malformed::ContentLengthAndChunked, 200, &[], b"abcd");
        let s = text(&r);
        assert!(s.contains("content-length: 4\r\n"));
        assert!(s.contains("transfer-encoding: chunked\r\n"));
        assert!(s.ends_with("\r\n\r\n4\r\nabcd\r\n0\r\n\r\n"));
        assert_eq!(r.framing, "content-length=4+chunked");
        assert!(r.must_close);
    }

    #[test]
    fn an_overstated_chunk_size_line_lies_upward() {
        let r = render_malformed(Malformed::ChunkSizeOverstated, 200, &[], b"abcd");
        let s = text(&r);
        // 4 bytes follow, the size line claims 0xc == 12.
        assert!(s.ends_with("\r\n\r\nc\r\nabcd\r\n0\r\n\r\n"), "{s}");
        assert_eq!(r.framing, "chunked;size-line=12;actual=4");
    }

    #[test]
    fn an_understated_chunk_size_line_lies_downward() {
        let r = render_malformed(Malformed::ChunkSizeUnderstated, 200, &[], b"abcd");
        let s = text(&r);
        assert!(s.ends_with("\r\n\r\n2\r\nabcd\r\n0\r\n\r\n"), "{s}");
        assert_eq!(r.framing, "chunked;size-line=2;actual=4");
    }

    #[test]
    fn a_missing_terminator_never_writes_the_zero_chunk() {
        let r = render_malformed(Malformed::ChunkTerminatorMissing, 200, &[], b"abcd");
        let s = text(&r);
        assert!(s.ends_with("\r\n\r\n4\r\nabcd\r\n"), "{s}");
        assert!(!s.ends_with("0\r\n\r\n"));
        assert_eq!(r.framing, "chunked;terminator=missing;actual=4");
        assert!(r.must_close);
    }

    #[test]
    fn a_corrupt_terminator_is_not_valid_hex() {
        let r = render_malformed(Malformed::ChunkTerminatorCorrupt, 200, &[], b"abcd");
        let s = text(&r);
        assert!(s.ends_with("\r\n\r\n4\r\nabcd\r\nZZ\r\n\r\n"), "{s}");
        assert!(u64::from_str_radix("ZZ", 16).is_err());
    }

    #[test]
    fn content_length_can_overstate_and_understate_the_bytes_written() {
        let over = render_malformed(Malformed::ContentLengthOverstated, 200, &[], b"abcd");
        let s = text(&over);
        assert!(s.contains("content-length: 20\r\n"));
        assert!(s.ends_with("\r\n\r\nabcd"));
        assert_eq!(over.framing, "content-length=20;actual=4");

        let under = render_malformed(Malformed::ContentLengthUnderstated, 200, &[], b"abcd");
        let s = text(&under);
        assert!(s.contains("content-length: 2\r\n"));
        assert!(s.ends_with("\r\n\r\nabcd"));
        assert_eq!(under.framing, "content-length=2;actual=4");
    }

    #[test]
    fn every_malformed_variant_forces_a_close() {
        for kind in [
            Malformed::ContentLengthAndChunked,
            Malformed::ChunkSizeOverstated,
            Malformed::ChunkSizeUnderstated,
            Malformed::ChunkTerminatorMissing,
            Malformed::ChunkTerminatorCorrupt,
            Malformed::ContentLengthOverstated,
            Malformed::ContentLengthUnderstated,
        ] {
            let r = render_malformed(kind, 200, &[], b"abcd");
            assert!(r.must_close, "{} should force a close", kind.as_str());
            assert!(!r.framing.is_empty());
        }
    }
}
