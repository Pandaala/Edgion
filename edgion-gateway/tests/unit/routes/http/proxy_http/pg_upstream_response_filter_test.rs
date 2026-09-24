use super::{normalize_final_response_headers, release_late_request_body_view};
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::{PluginSession, StreamingResponseProcessor};
use crate::request_body::BodyStorage;
use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::EdgionHttpContext;
use async_trait::async_trait;
use bytes::Bytes;
use pingora_core::protocols::http::RequestBodyBuffer;
use pingora_http::ResponseHeader;
use pingora_proxy::ResponseBodySink;

struct ProviderHeaderReset;

#[async_trait]
impl StreamingResponseProcessor for ProviderHeaderReset {
    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        response.remove_header("server");
        response.remove_header("strict-transport-security");
    }

    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<Bytes>,
        _end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        _sink: &mut ResponseBodySink,
        _session: &mut dyn PluginSession,
    ) {
    }
}

#[test]
fn gateway_server_headers_are_applied_after_provider_header_reset() {
    let mut tls_response = ResponseHeader::build(200, None).unwrap();
    tls_response.insert_header("server", "provider").unwrap();
    tls_response
        .insert_header("strict-transport-security", "max-age=1")
        .unwrap();
    let mut tls_ctx = EdgionHttpContext::default();
    tls_ctx.push_response_stream_processor(Box::new(ProviderHeaderReset));

    normalize_final_response_headers(&mut tls_response, &mut tls_ctx, &ServerHeaderOpts::default());

    assert_eq!(tls_response.headers["server"], "Edgion");
    assert_eq!(
        tls_response.headers["strict-transport-security"],
        "max-age=63072000; includeSubDomains; preload"
    );

    let mut plain_response = ResponseHeader::build(200, None).unwrap();
    plain_response.insert_header("server", "provider").unwrap();
    plain_response
        .insert_header("strict-transport-security", "max-age=1")
        .unwrap();
    let mut plain_ctx = EdgionHttpContext::default();
    plain_ctx.push_response_stream_processor(Box::new(ProviderHeaderReset));
    let mut plain_headers = ServerHeaderOpts::default();
    plain_headers.remove_hsts();

    normalize_final_response_headers(&mut plain_response, &mut plain_ctx, &plain_headers);

    assert_eq!(plain_response.headers["server"], "Edgion");
    assert!(plain_response.headers.get("strict-transport-security").is_none());
}

#[tokio::test]
async fn final_response_header_stage_releases_request_body_view() {
    let (mut storage, view) = BodyStorage::new(64, 64);
    storage.write(&Bytes::from_static(b"cached")).await.unwrap();
    storage.finish().await.unwrap();
    let mut request_body = crate::request_body::RequestBodyState::default();
    request_body.install(view);
    request_body.overwrite(Bytes::from_static(b"edited")).unwrap();

    let mut ctx = EdgionHttpContext::default();
    ctx.request_body = request_body;
    assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(6));
    release_late_request_body_view(&mut ctx);

    assert!(!ctx.request_body.has_cached_body());
    assert_eq!(ctx.request_body.modified_len_for_framing().unwrap(), Some(6));
}
