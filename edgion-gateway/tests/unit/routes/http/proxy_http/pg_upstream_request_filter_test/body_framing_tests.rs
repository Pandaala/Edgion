use super::sync_body_framing_headers;
use crate::request_body::BodyStorage;
use crate::EdgionHttpContext;
use bytes::Bytes;
use pingora_core::protocols::http::RequestBodyBuffer;

#[tokio::test]
async fn mutation_sets_content_length_and_removes_transfer_encoding() {
    let (mut storage, view) = BodyStorage::new(64, 64);
    storage.write(&Bytes::from_static(b"old")).await.unwrap();
    storage.finish().await.unwrap();
    let mut request_body = crate::request_body::RequestBodyState::default();
    request_body.install(view);
    request_body.overwrite(Bytes::from_static(b"new-body")).unwrap();

    let mut ctx = EdgionHttpContext::default();
    ctx.request_body = request_body;
    let mut request = pingora_http::RequestHeader::build("POST", b"/", None).unwrap();
    request.insert_header("transfer-encoding", "chunked").unwrap();
    request.insert_header("content-length", "3").unwrap();

    sync_body_framing_headers(&mut request, &mut ctx).unwrap();
    assert_eq!(request.headers.get("content-length").unwrap(), "8");
    assert!(!request.headers.contains_key("transfer-encoding"));

    ctx.request_body.release_view();
    let mut retry_request = pingora_http::RequestHeader::build("POST", b"/", None).unwrap();
    retry_request.insert_header("transfer-encoding", "chunked").unwrap();
    retry_request.insert_header("content-length", "3").unwrap();

    sync_body_framing_headers(&mut retry_request, &mut ctx).unwrap();
    assert_eq!(retry_request.headers.get("content-length").unwrap(), "8");
    assert!(!retry_request.headers.contains_key("transfer-encoding"));
}
