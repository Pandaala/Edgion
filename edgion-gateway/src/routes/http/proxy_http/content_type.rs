//! Shared `Content-Type` media-type detection helpers for gRPC / gRPC-Web.
//!
//! Per RFC 9110 §8.3.1 the media type `type` and `subtype` are matched
//! case-insensitively, so `Application/Grpc` and `APPLICATION/GRPC-WEB+PROTO`
//! are equivalent to their lowercase forms. Centralizing the checks here keeps
//! the early-filter gRPC-Web bridge activation and the later protocol-detection
//! path from drifting apart.

/// gRPC-Web media types all share this lowercase prefix
/// (`application/grpc-web`, `application/grpc-web+proto`, ...).
const GRPC_WEB_PREFIX: &str = "application/grpc-web";

/// Native gRPC media types all share this lowercase prefix
/// (`application/grpc`, `application/grpc+proto`, ...). Note this prefix also
/// matches gRPC-Web, so callers that need to distinguish the two MUST test
/// [`is_grpc_web_content_type`] first.
const GRPC_PREFIX: &str = "application/grpc";

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
}
