// Header-steered upstream scripts.
//
// The control headers are read from the request the *gateway forwards*, so a
// test client sets them on its original request and they survive the proxy.
// The `x-up-` prefix follows the repo's existing `x-…` control-header idiom
// (`x-ext-proc-test-mode`, `x-trace-id`) and does not collide with the
// `x-up-req-*` framing-report *response* headers.
//
//   x-up-behavior        which script this request runs (see `Behavior`)
//   x-up-at-bytes        request-body byte offset at which the action fires
//   x-up-reset-code      H2 error code for `reset` / `after-response: reset`
//   x-up-reset-first     reset the first N requests for a trace before gRPC success
//   x-up-after-response  what the upstream does after a complete response
//   x-up-delay-ms        delay before the response head is written (cap 10 000)
//   x-up-status          response status, default 200
//   x-up-resp-len        response body length, default 16 (cap 65 536)
//   x-up-response-trailers emit one fixed, test-only response trailer (default false)
//   x-up-drain           keep reading the request body after acting (default true)
//   x-trace-id           the recorder key — the existing project idiom

/// Maximum honoured `x-up-delay-ms`.
pub const MAX_DELAY_MS: u64 = 10_000;
/// Maximum honoured `x-up-resp-len`.
pub const MAX_RESP_LEN: usize = 65_536;
/// Default response body length when `x-up-resp-len` is absent.
pub const DEFAULT_RESP_LEN: usize = 16;
pub const MAX_RESET_FIRST: usize = 8;

/// Deliberately malformed HTTP/1 response framings.
///
/// Named for the intent, not the byte recipe, so a case reads as a claim. All
/// of these are HTTP/1-only: the `h2` crate refuses to put an equivalent on the
/// wire, and HTTP/2 has no chunked framing to corrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// `Content-Length` *and* `Transfer-Encoding: chunked` on the same response.
    /// The classic request/response-smuggling disagreement primitive.
    ContentLengthAndChunked,
    /// A chunk-size line that claims more bytes than the chunk actually carries.
    ChunkSizeOverstated,
    /// A chunk-size line that claims fewer bytes than the chunk actually carries,
    /// so the trailing bytes are read as the next chunk-size line.
    ChunkSizeUnderstated,
    /// A chunked body that simply stops — no terminal `0\r\n\r\n`.
    ChunkTerminatorMissing,
    /// A terminal chunk whose size line is not valid hex.
    ChunkTerminatorCorrupt,
    /// `Content-Length` larger than the number of body bytes actually written.
    ContentLengthOverstated,
    /// `Content-Length` smaller than the number of body bytes actually written.
    ContentLengthUnderstated,
}

impl Malformed {
    pub fn as_str(self) -> &'static str {
        match self {
            Malformed::ContentLengthAndChunked => "malformed-cl-and-te",
            Malformed::ChunkSizeOverstated => "malformed-chunk-size-overstated",
            Malformed::ChunkSizeUnderstated => "malformed-chunk-size-understated",
            Malformed::ChunkTerminatorMissing => "malformed-chunk-terminator-missing",
            Malformed::ChunkTerminatorCorrupt => "malformed-chunk-terminator-corrupt",
            Malformed::ContentLengthOverstated => "malformed-content-length-overstated",
            Malformed::ContentLengthUnderstated => "malformed-content-length-understated",
        }
    }
}

/// The script an individual request runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    /// Read the whole body, then respond with the framing report headers.
    Echo,
    /// Respond at `at_bytes`, before the request body is complete.
    EarlyResponse,
    /// Read the body, then never respond.
    Hang,
    /// At `at_bytes`, FIN the write half (H1) / finish the response and drop the
    /// receive stream (H2), then keep reading.
    HalfClose,
    /// At `at_bytes`, reset: H1 sends a real TCP RST (`SO_LINGER(0)` + drop),
    /// H2 sends RST_STREAM with `reset_code`.
    Reset,
    /// At `at_bytes`, close the connection gracefully without responding.
    Close,
    /// Respond with a deliberately malformed framing (HTTP/1 only).
    Malformed(Malformed),
    /// H2-only valid unary `test.TestService/SayHello` response.
    GrpcSayHello,
    /// H1 only: emit `100 Continue` and hang on the first trace-keyed request,
    /// then serve later requests with the ordinary bounded echo response.
    InformationalThenHangFirst,
}

impl Behavior {
    pub fn as_str(self) -> &'static str {
        match self {
            Behavior::Echo => "echo",
            Behavior::EarlyResponse => "early-response",
            Behavior::Hang => "hang",
            Behavior::HalfClose => "half-close",
            Behavior::Reset => "reset",
            Behavior::Close => "close",
            Behavior::Malformed(m) => m.as_str(),
            Behavior::GrpcSayHello => "grpc-say-hello",
            Behavior::InformationalThenHangFirst => "informational-then-hang-first",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().as_str() {
            "echo" => Behavior::Echo,
            "early-response" => Behavior::EarlyResponse,
            "hang" => Behavior::Hang,
            "half-close" => Behavior::HalfClose,
            "reset" => Behavior::Reset,
            "close" => Behavior::Close,
            "grpc-say-hello" => Behavior::GrpcSayHello,
            "informational-then-hang-first" => Behavior::InformationalThenHangFirst,
            "malformed-cl-and-te" => Behavior::Malformed(Malformed::ContentLengthAndChunked),
            "malformed-chunk-size-overstated" => Behavior::Malformed(Malformed::ChunkSizeOverstated),
            "malformed-chunk-size-understated" => Behavior::Malformed(Malformed::ChunkSizeUnderstated),
            "malformed-chunk-terminator-missing" => Behavior::Malformed(Malformed::ChunkTerminatorMissing),
            "malformed-chunk-terminator-corrupt" => Behavior::Malformed(Malformed::ChunkTerminatorCorrupt),
            "malformed-content-length-overstated" => Behavior::Malformed(Malformed::ContentLengthOverstated),
            "malformed-content-length-understated" => Behavior::Malformed(Malformed::ContentLengthUnderstated),
            _ => return None,
        })
    }
}

/// What the upstream does once a complete response has been written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterResponse {
    /// Keep the connection usable (H1 keep-alive / H2 connection untouched).
    /// This is the default, because otherwise no upstream connection could ever
    /// be reused and the pool-reuse claims would be untestable.
    Linger,
    /// Close the connection gracefully (H1 sends `Connection: close` + FIN).
    Close,
    /// H1: real TCP RST after the response. H2: RST_STREAM with `reset_code`
    /// after a *complete* response — the benign post-EOF reset shape.
    Reset,
    /// H2: GOAWAY after the response. H1 has no GOAWAY, so it degrades to
    /// `Close` and records the degradation in the `RespHead` detail.
    Goaway,
}

impl AfterResponse {
    pub fn as_str(self) -> &'static str {
        match self {
            AfterResponse::Linger => "linger",
            AfterResponse::Close => "close",
            AfterResponse::Reset => "reset",
            AfterResponse::Goaway => "goaway",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().as_str() {
            "linger" => AfterResponse::Linger,
            "close" => AfterResponse::Close,
            "reset" => AfterResponse::Reset,
            "goaway" => AfterResponse::Goaway,
            _ => return None,
        })
    }
}

/// The fully-resolved script for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpScript {
    pub behavior: Behavior,
    pub at_bytes: usize,
    pub reset_code: String,
    pub reset_first: usize,
    pub after_response: AfterResponse,
    pub delay_ms: u64,
    pub status: u16,
    pub resp_len: usize,
    pub response_trailers: bool,
    pub drain: bool,
    pub trace_id: Option<String>,
    /// Set when a control header carried a value this backend does not know.
    /// Surfaced in the `RespHead` event and as an `x-up-script-warning`
    /// response header, so a typo cannot silently degrade to `echo` and make a
    /// case pass vacuously.
    pub warning: Option<String>,
}

impl Default for UpScript {
    fn default() -> Self {
        Self {
            behavior: Behavior::Echo,
            at_bytes: 0,
            reset_code: "NO_ERROR".to_string(),
            reset_first: 0,
            after_response: AfterResponse::Linger,
            delay_ms: 0,
            status: 200,
            resp_len: DEFAULT_RESP_LEN,
            response_trailers: false,
            drain: true,
            trace_id: None,
            warning: None,
        }
    }
}

impl UpScript {
    /// Resolve the script from a header lookup closure. Both backends supply
    /// their own lookup (H1 parses raw bytes, H2 reads an `http::HeaderMap`),
    /// so the vocabulary lives in exactly one place.
    pub fn from_headers<F>(get: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut script = UpScript::default();
        let mut warnings: Vec<String> = Vec::new();

        if let Some(raw) = get("x-up-behavior") {
            match Behavior::parse(&raw) {
                Some(b) => script.behavior = b,
                None => warnings.push(format!("unknown x-up-behavior '{}', using echo", raw)),
            }
        }
        if let Some(raw) = get("x-up-at-bytes") {
            match raw.trim().parse::<usize>() {
                Ok(v) => script.at_bytes = v,
                Err(_) => warnings.push(format!("unparsable x-up-at-bytes '{}', using 0", raw)),
            }
        }
        if let Some(raw) = get("x-up-reset-code") {
            script.reset_code = raw.trim().to_ascii_uppercase();
        }
        if let Some(raw) = get("x-up-reset-first") {
            match raw.trim().parse::<usize>() {
                Ok(v) => script.reset_first = v.min(MAX_RESET_FIRST),
                Err(_) => warnings.push(format!("unparsable x-up-reset-first '{}', using 0", raw)),
            }
        }
        if let Some(raw) = get("x-up-after-response") {
            match AfterResponse::parse(&raw) {
                Some(a) => script.after_response = a,
                None => warnings.push(format!("unknown x-up-after-response '{}', using linger", raw)),
            }
        }
        if let Some(raw) = get("x-up-delay-ms") {
            match raw.trim().parse::<u64>() {
                Ok(v) => script.delay_ms = v.min(MAX_DELAY_MS),
                Err(_) => warnings.push(format!("unparsable x-up-delay-ms '{}', using 0", raw)),
            }
        }
        if let Some(raw) = get("x-up-status") {
            match raw.trim().parse::<u16>() {
                Ok(v) if (100..=599).contains(&v) => script.status = v,
                _ => warnings.push(format!("unusable x-up-status '{}', using 200", raw)),
            }
        }
        if let Some(raw) = get("x-up-resp-len") {
            match raw.trim().parse::<usize>() {
                Ok(v) => script.resp_len = v.min(MAX_RESP_LEN),
                Err(_) => warnings.push(format!("unparsable x-up-resp-len '{}', using default", raw)),
            }
        }
        if let Some(raw) = get("x-up-response-trailers") {
            match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => script.response_trailers = true,
                "false" | "0" | "no" => script.response_trailers = false,
                other => warnings.push(format!("unknown x-up-response-trailers '{}', using false", other)),
            }
        }
        if let Some(raw) = get("x-up-drain") {
            match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => script.drain = true,
                "false" | "0" | "no" => script.drain = false,
                other => warnings.push(format!("unknown x-up-drain '{}', using true", other)),
            }
        }
        script.trace_id = get("x-trace-id")
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());

        if !warnings.is_empty() {
            script.warning = Some(warnings.join("; "));
        }
        script
    }

    /// The response body this script emits: a repeating printable pattern so a
    /// truncation is visible in a hexdump.
    pub fn response_body(&self) -> Vec<u8> {
        const PATTERN: &[u8] = b"edgion-transport-";
        (0..self.resp_len).map(|i| PATTERN[i % PATTERN.len()]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script_from(pairs: &[(&str, &str)]) -> UpScript {
        let owned: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        UpScript::from_headers(|name| {
            owned
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn defaults_are_echo_linger_200() {
        let s = script_from(&[]);
        assert_eq!(s.behavior, Behavior::Echo);
        assert_eq!(s.after_response, AfterResponse::Linger);
        assert_eq!(s.status, 200);
        assert_eq!(s.at_bytes, 0);
        assert!(s.drain);
        assert_eq!(s.resp_len, DEFAULT_RESP_LEN);
        assert!(!s.response_trailers);
        assert!(s.warning.is_none());
        assert!(s.trace_id.is_none());
    }

    #[test]
    fn every_behavior_name_round_trips() {
        let names = [
            "echo",
            "early-response",
            "hang",
            "half-close",
            "reset",
            "close",
            "grpc-say-hello",
            "informational-then-hang-first",
            "malformed-cl-and-te",
            "malformed-chunk-size-overstated",
            "malformed-chunk-size-understated",
            "malformed-chunk-terminator-missing",
            "malformed-chunk-terminator-corrupt",
            "malformed-content-length-overstated",
            "malformed-content-length-understated",
        ];
        for name in names {
            let b = Behavior::parse(name).unwrap_or_else(|| panic!("{name} did not parse"));
            assert_eq!(b.as_str(), name);
        }
    }

    #[test]
    fn reset_first_parses_and_caps_the_trace_attempt_count() {
        assert_eq!(script_from(&[("x-up-reset-first", "2")]).reset_first, 2);
        assert_eq!(script_from(&[("x-up-reset-first", "999")]).reset_first, MAX_RESET_FIRST);
        let invalid = script_from(&[("x-up-reset-first", "two")]);
        assert_eq!(invalid.reset_first, 0);
        assert!(invalid
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("x-up-reset-first")));
    }

    #[test]
    fn the_post_response_reset_shape_parses() {
        let s = script_from(&[
            ("x-up-behavior", "echo"),
            ("x-up-after-response", "reset"),
            ("x-up-reset-code", "no_error"),
            ("x-trace-id", "t-abc"),
        ]);
        assert_eq!(s.behavior, Behavior::Echo);
        assert_eq!(s.after_response, AfterResponse::Reset);
        assert_eq!(s.reset_code, "NO_ERROR");
        assert_eq!(s.trace_id.as_deref(), Some("t-abc"));
    }

    #[test]
    fn delay_and_resp_len_are_capped() {
        let s = script_from(&[("x-up-delay-ms", "999999"), ("x-up-resp-len", "99999999")]);
        assert_eq!(s.delay_ms, MAX_DELAY_MS);
        assert_eq!(s.resp_len, MAX_RESP_LEN);
    }

    #[test]
    fn an_unknown_behavior_warns_instead_of_silently_echoing() {
        let s = script_from(&[("x-up-behavior", "eco")]);
        assert_eq!(s.behavior, Behavior::Echo);
        assert!(s.warning.as_deref().unwrap().contains("unknown x-up-behavior 'eco'"));
    }

    #[test]
    fn an_out_of_range_status_warns() {
        let s = script_from(&[("x-up-status", "999")]);
        assert_eq!(s.status, 200);
        assert!(s.warning.is_some());
    }

    #[test]
    fn drain_accepts_the_usual_spellings() {
        assert!(!script_from(&[("x-up-drain", "false")]).drain);
        assert!(!script_from(&[("x-up-drain", "0")]).drain);
        assert!(script_from(&[("x-up-drain", "yes")]).drain);
        assert!(script_from(&[("x-up-drain", "maybe")]).warning.is_some());
    }

    #[test]
    fn response_trailers_are_explicit_and_fail_closed_on_typos() {
        assert!(script_from(&[("x-up-response-trailers", "true")]).response_trailers);
        let typo = script_from(&[("x-up-response-trailers", "tru")]);
        assert!(!typo.response_trailers);
        assert!(typo.warning.is_some());
    }

    #[test]
    fn response_body_has_the_requested_length() {
        let s = script_from(&[("x-up-resp-len", "40")]);
        assert_eq!(s.response_body().len(), 40);
        assert_eq!(script_from(&[("x-up-resp-len", "0")]).response_body(), Vec::<u8>::new());
    }
}
