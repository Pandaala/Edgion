use std::time::Duration;

use edgion_common::link_sys_core::http_client::{
    apply_outbound_policy, configure_outbound_http2_max_header_list_size, OutboundClientConfig,
    MAX_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE,
};
use http::{HeaderValue, Response};
use reqwest::Client;

async fn request_h2_response_with_decoded_header_size(decoded_header_list_size: u32) -> reqwest::Result<()> {
    // h2 counts each field as name bytes + value bytes + 32 bytes. This
    // response has `:status` (7 + 3 + 32) and `x-pad` (5 + value + 32).
    const FIXED_DECODED_SIZE: u32 = 79;
    let value_len = decoded_header_list_size.checked_sub(FIXED_DECODED_SIZE).unwrap() as usize;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        drop(request);
        let response = Response::builder()
            .status(200)
            .header("x-pad", HeaderValue::from_bytes(&vec![b'a'; value_len]).unwrap())
            .body(())
            .unwrap();
        respond.send_response(response, true).unwrap();

        // Poll the connection until the response is flushed. The short timeout
        // also bounds cleanup when the client keeps its H2 connection pooled.
        let _ = tokio::time::timeout(Duration::from_millis(100), connection.accept()).await;
    });

    let client = apply_outbound_policy(
        Client::builder().http2_prior_knowledge(),
        OutboundClientConfig {
            disable_proxy: true,
            ..Default::default()
        },
    )
    .unwrap()
    .build()
    .unwrap();
    let result = client.get(format!("http://{address}/")).send().await.map(|_| ());
    server.await.unwrap();
    result
}

#[tokio::test]
async fn canonical_client_rejects_the_exact_decoded_header_boundary() {
    let limit = MAX_OUTBOUND_HTTP2_MAX_HEADER_LIST_SIZE;
    configure_outbound_http2_max_header_list_size(limit).unwrap();

    let below = request_h2_response_with_decoded_header_size(limit - 1).await;
    assert!(below.is_ok(), "header list below the limit failed: {below:?}");
    assert!(request_h2_response_with_decoded_header_size(limit).await.is_err());
}
