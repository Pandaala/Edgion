//! Shared `Content-Type` media-type detection helpers for gRPC / gRPC-Web / SSE.
//!
//! Per RFC 9110 §8.3.1 the media type `type` and `subtype` are matched
//! case-insensitively, so `Application/Grpc` and `APPLICATION/GRPC-WEB+PROTO`
//! are equivalent to their lowercase forms. Centralizing the checks here keeps
//! the early-filter gRPC-Web bridge activation and the later protocol-detection
//! path from drifting apart.

use crate::ctx::ResponseStreamKind;
use http::header::CONTENT_ENCODING;

/// SSE media type, optionally followed by parameters such as
/// `; charset=utf-8`.
const EVENT_STREAM_MEDIA_TYPE: &str = "text/event-stream";

/// gRPC-Web media types all share this lowercase prefix
/// (`application/grpc-web`, `application/grpc-web+proto`, ...).
const GRPC_WEB_PREFIX: &str = "application/grpc-web";

/// Native gRPC media types all share this lowercase prefix
/// (`application/grpc`, `application/grpc+proto`, ...). Note this prefix also
/// matches gRPC-Web, so callers that need to distinguish the two MUST test
/// [`is_grpc_web_content_type`] first.
const GRPC_PREFIX: &str = "application/grpc";

/// Classify the upstream response's stream kind from its `Content-Type`.
/// Explicit media types only: anything unrecognized is `None`.
#[inline]
pub fn classify_response_stream_kind(resp: &pingora_http::ResponseHeader) -> ResponseStreamKind {
    let Some(ct) = resp.headers.get(http::header::CONTENT_TYPE) else {
        return ResponseStreamKind::None;
    };
    let Ok(ct_str) = std::str::from_utf8(ct.as_bytes()) else {
        return ResponseStreamKind::None;
    };
    if is_sse_content_type(ct_str) {
        ResponseStreamKind::Sse
    } else {
        ResponseStreamKind::None
    }
}

/// Returns `true` when `content_type` is the exact SSE media type,
/// case-insensitively, with optional whitespace and parameters.
#[inline]
pub(crate) fn is_sse_content_type(content_type: &str) -> bool {
    matches_media_type(content_type, EVENT_STREAM_MEDIA_TYPE)
}

/// Returns `true` when the response has no content coding or every coding is
/// `identity`. All repeated field lines and comma-delimited list members are
/// classified without allocating owned header values. Empty list members are
/// ignored per RFC 9110 section 5.6.1.2, but a present field must contain at
/// least one effective coding.
#[inline]
pub(crate) fn response_is_identity_encoded(headers: &http::HeaderMap) -> bool {
    let mut values = headers.get_all(CONTENT_ENCODING).iter().peekable();
    if values.peek().is_none() {
        return true;
    }

    let mut saw_coding = false;
    for value in values {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for coding in value.split(',').map(str::trim).filter(|coding| !coding.is_empty()) {
            saw_coding = true;
            if !coding.eq_ignore_ascii_case("identity") {
                return false;
            }
        }
    }
    saw_coding
}

/// Match one exact media type case-insensitively, allowing optional whitespace
/// and parameters after it without accepting a longer subtype with the same
/// prefix.
#[inline]
fn matches_media_type(value: &str, media_type: &str) -> bool {
    if !starts_with_ignore_ascii_case(value, media_type) {
        return false;
    }
    let suffix = &value.as_bytes()[media_type.len()..];
    let suffix = suffix
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(&[][..], |first| &suffix[first..]);
    suffix.is_empty() || suffix.first() == Some(&b';')
}

/// Returns `true` when `content_type` is a gRPC-Web media type, comparing the
/// prefix case-insensitively. Trailing parameters (e.g. `+proto`,
/// `; charset=utf-8`) are tolerated by the prefix match.
#[inline]
pub fn is_grpc_web_content_type(content_type: &str) -> bool {
    starts_with_ignore_ascii_case(content_type, GRPC_WEB_PREFIX)
}

/// Returns `true` when `content_type` is a native gRPC media type, comparing
/// the prefix case-insensitively. Because this also matches gRPC-Web, check
/// [`is_grpc_web_content_type`] first when the two must be told apart.
#[inline]
pub fn is_grpc_content_type(content_type: &str) -> bool {
    starts_with_ignore_ascii_case(content_type, GRPC_PREFIX)
}

/// Case-insensitive (ASCII) prefix check that avoids allocating a lowercased
/// copy of `haystack`.
#[inline]
fn starts_with_ignore_ascii_case(haystack: &str, prefix: &str) -> bool {
    haystack.len() >= prefix.len() && haystack.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn content_encoding_headers(values: &[&[u8]]) -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        for value in values {
            headers.append(CONTENT_ENCODING, HeaderValue::from_bytes(value).unwrap());
        }
        headers
    }

    #[test]
    fn identity_encoding_classifies_every_field_and_list_member() {
        for accepted in [
            &[][..],
            &[b"identity".as_slice()][..],
            &[b" IDENTITY \t".as_slice()][..],
            &[b"identity".as_slice(), b"Identity".as_slice()][..],
            &[b"identity, IDENTITY".as_slice()][..],
            &[b"identity, identity".as_slice(), b"IDENTITY".as_slice()][..],
            &[b"identity,".as_slice()][..],
            &[b", identity, , IDENTITY,".as_slice()][..],
            &[b",".as_slice(), b"identity".as_slice(), b", ,".as_slice()][..],
        ] {
            assert!(response_is_identity_encoded(&content_encoding_headers(accepted)));
        }

        for rejected in [
            &[b"gzip".as_slice()][..],
            &[b"identity".as_slice(), b"gzip".as_slice()][..],
            &[b"gzip".as_slice(), b"identity".as_slice()][..],
            &[b"identity, gzip".as_slice()][..],
            &[b"gzip, identity".as_slice()][..],
            &[b"".as_slice()][..],
            &[b", ,".as_slice()][..],
            &[&[0xff][..]][..],
        ] {
            assert!(!response_is_identity_encoded(&content_encoding_headers(rejected)));
        }
    }

    #[test]
    fn native_grpc_matches_various_casings_and_params() {
        for ct in [
            "application/grpc",
            "Application/Grpc",
            "APPLICATION/GRPC",
            "application/grpc+proto",
            "application/grpc; charset=utf-8",
        ] {
            assert!(is_grpc_content_type(ct), "expected gRPC match for {ct:?}");
        }
    }

    #[test]
    fn grpc_web_matches_various_casings_and_params() {
        for ct in [
            "application/grpc-web",
            "application/grpc-web+proto",
            "APPLICATION/GRPC-WEB+PROTO",
            "Application/Grpc-Web",
        ] {
            assert!(is_grpc_web_content_type(ct), "expected gRPC-Web match for {ct:?}");
        }
    }

    #[test]
    fn grpc_web_is_also_matched_by_native_prefix() {
        // The native prefix is a substring of the gRPC-Web prefix, so callers
        // must test gRPC-Web first; this guards that ordering contract.
        assert!(is_grpc_content_type("application/grpc-web"));
    }

    #[test]
    fn non_grpc_content_types_do_not_match() {
        for ct in [
            "application/json",
            "text/plain",
            "application/grp", // shorter than the prefix
            "",
        ] {
            assert!(!is_grpc_content_type(ct), "unexpected gRPC match for {ct:?}");
            assert!(!is_grpc_web_content_type(ct), "unexpected gRPC-Web match for {ct:?}");
        }
    }

    #[test]
    fn event_stream_classification_matches_casings_and_params() {
        use crate::ctx::ResponseStreamKind;
        for ct in [
            "text/event-stream",
            "Text/Event-Stream",
            "TEXT/EVENT-STREAM",
            "text/event-stream; charset=utf-8",
            "text/event-stream \t; charset=utf-8",
        ] {
            assert!(is_sse_content_type(ct), "expected SSE media type match for {ct:?}");
            let mut resp = pingora_http::ResponseHeader::build(200, None).unwrap();
            resp.insert_header("content-type", ct).unwrap();
            assert_eq!(
                classify_response_stream_kind(&resp),
                ResponseStreamKind::Sse,
                "expected SSE for {ct:?}"
            );
        }
    }

    #[test]
    fn non_event_stream_and_missing_content_type_classify_as_none() {
        use crate::ctx::ResponseStreamKind;
        let bare = pingora_http::ResponseHeader::build(200, None).unwrap();
        assert_eq!(classify_response_stream_kind(&bare), ResponseStreamKind::None);
        for ct in [
            "application/json",
            "text/plain",
            "application/grpc",
            "text/event-streaming",
            "text/event-stream+json",
            "",
        ] {
            assert!(!is_sse_content_type(ct), "unexpected SSE media type match for {ct:?}");
            let mut resp = pingora_http::ResponseHeader::build(200, None).unwrap();
            resp.insert_header("content-type", ct).unwrap();
            assert_eq!(
                classify_response_stream_kind(&resp),
                ResponseStreamKind::None,
                "unexpected streaming classification for {ct:?}"
            );
        }
    }
}
