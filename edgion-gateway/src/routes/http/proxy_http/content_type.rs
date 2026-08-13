//! Shared `Content-Type` media-type detection helpers for gRPC / gRPC-Web / SSE.
//!
//! Per RFC 9110 §8.3.1 the media type `type` and `subtype` are matched
//! case-insensitively, so `Application/Grpc` and `APPLICATION/GRPC-WEB+PROTO`
//! are equivalent to their lowercase forms. Centralizing the checks here keeps
//! the early-filter gRPC-Web bridge activation and the later protocol-detection
//! path from drifting apart.

use crate::ctx::ResponseStreamKind;
use crate::plugins::http::a2a_gateway::A2aResponseMedia;

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
    if matches_media_type(ct_str, EVENT_STREAM_MEDIA_TYPE) {
        ResponseStreamKind::Sse
    } else {
        ResponseStreamKind::None
    }
}

/// Classify the final, post-plugin response representation that the read-only
/// A2A observer may inspect. Encoded or bodyless representations are opaque:
/// observation must never decompress, reinterpret, or invent response bytes.
#[inline]
pub(crate) fn classify_a2a_response_media(
    method: &http::Method,
    resp: &pingora_http::ResponseHeader,
) -> A2aResponseMedia {
    if *method == http::Method::HEAD
        || resp.status.is_informational()
        || matches!(
            resp.status,
            http::StatusCode::NO_CONTENT | http::StatusCode::NOT_MODIFIED
        )
        || !has_identity_content_encoding(resp)
    {
        return A2aResponseMedia::Other;
    }
    let mut content_types = resp.headers.get_all(http::header::CONTENT_TYPE).iter();
    let Some(content_type) = content_types.next().and_then(|value| value.to_str().ok()) else {
        return A2aResponseMedia::Other;
    };
    if content_types.next().is_some() {
        return A2aResponseMedia::Other;
    }
    if matches_media_type(content_type, EVENT_STREAM_MEDIA_TYPE) {
        A2aResponseMedia::Sse
    } else if is_supported_json_media_type(content_type) {
        A2aResponseMedia::Json
    } else {
        A2aResponseMedia::Other
    }
}

#[inline]
fn has_identity_content_encoding(resp: &pingora_http::ResponseHeader) -> bool {
    let mut values = resp.headers.get_all(http::header::CONTENT_ENCODING).iter();
    let Some(first) = values.next() else {
        return true;
    };
    values.next().is_none()
        && first
            .to_str()
            .ok()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("identity"))
}

#[inline]
fn is_supported_json_media_type(value: &str) -> bool {
    let Ok(media_type) = value.trim().parse::<mime::Mime>() else {
        return false;
    };
    let subtype = media_type.subtype();
    let is_json = subtype == mime::JSON || (subtype.as_str() != "*" && media_type.suffix() == Some(mime::JSON));
    if media_type.type_() != mime::APPLICATION || !is_json {
        return false;
    }
    let mut charset_count = 0usize;
    for (name, value) in media_type.params() {
        if name != mime::CHARSET {
            return false;
        }
        charset_count += 1;
        if charset_count > 1 || !value.as_str().eq_ignore_ascii_case("utf-8") {
            return false;
        }
    }
    true
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

    fn a2a_response(content_type: &str) -> pingora_http::ResponseHeader {
        let mut response = pingora_http::ResponseHeader::build(200, None).unwrap();
        response.insert_header("content-type", content_type).unwrap();
        response
    }

    #[test]
    fn a2a_media_accepts_exact_json_suffix_and_sse() {
        for content_type in [
            "application/json",
            "Application/JSON; charset=utf-8",
            "application/problem+json",
        ] {
            assert_eq!(
                classify_a2a_response_media(&http::Method::POST, &a2a_response(content_type)),
                A2aResponseMedia::Json
            );
        }
        assert_eq!(
            classify_a2a_response_media(&http::Method::POST, &a2a_response("text/event-stream; charset=utf-8")),
            A2aResponseMedia::Sse
        );
    }

    #[test]
    fn a2a_media_rejects_lookalikes_encoding_and_bodyless_responses() {
        for content_type in [
            "text/json",
            "application/json-seq",
            "application/notjson",
            "application/json; version=1",
            "application/json; charset=latin1",
            "text/event-streaming",
        ] {
            assert_eq!(
                classify_a2a_response_media(&http::Method::POST, &a2a_response(content_type)),
                A2aResponseMedia::Other
            );
        }

        let mut encoded = a2a_response("application/json");
        encoded.insert_header("content-encoding", "gzip").unwrap();
        assert_eq!(
            classify_a2a_response_media(&http::Method::POST, &encoded),
            A2aResponseMedia::Other
        );
        encoded.insert_header("content-encoding", "identity").unwrap();
        assert_eq!(
            classify_a2a_response_media(&http::Method::POST, &encoded),
            A2aResponseMedia::Json
        );

        let mut duplicate_type = a2a_response("application/json");
        duplicate_type
            .append_header("content-type", "application/json")
            .unwrap();
        assert_eq!(
            classify_a2a_response_media(&http::Method::POST, &duplicate_type),
            A2aResponseMedia::Other
        );
        let mut duplicate_encoding = a2a_response("application/json");
        duplicate_encoding
            .append_header("content-encoding", "identity")
            .unwrap();
        duplicate_encoding
            .append_header("content-encoding", "identity")
            .unwrap();
        assert_eq!(
            classify_a2a_response_media(&http::Method::POST, &duplicate_encoding),
            A2aResponseMedia::Other
        );

        assert_eq!(
            classify_a2a_response_media(&http::Method::HEAD, &a2a_response("application/json")),
            A2aResponseMedia::Other
        );
        let mut no_content = a2a_response("application/json");
        no_content.set_status(http::StatusCode::NO_CONTENT).unwrap();
        assert_eq!(
            classify_a2a_response_media(&http::Method::POST, &no_content),
            A2aResponseMedia::Other
        );
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
