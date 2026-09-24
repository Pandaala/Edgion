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
