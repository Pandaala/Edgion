use super::*;
use crate::request_body::BodyReadError;
use crate::request_body::BodyStorage;
use pingora_core::protocols::http::RequestBodyBuffer;

async fn context_with_body(body: &'static [u8], cap: u64) -> EdgionHttpContext {
    let (mut storage, view) = BodyStorage::new(128 * 1024, cap);
    storage.write(&Bytes::from_static(body)).await.unwrap();
    storage.finish().await.unwrap();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = cap;
    ctx.request_body.install(view);
    ctx
}

#[tokio::test]
async fn overwrite_enforces_physical_cap_atomically() {
    let mut exact = context_with_body(b"old", 4).await;
    exact.overwrite_request_body(Bytes::from_static(b"1234")).unwrap();
    assert_eq!(
        exact.request_body.read_all().await.unwrap().unwrap(),
        Bytes::from_static(b"1234")
    );

    let mut over = context_with_body(b"old", 4).await;
    assert_eq!(
        over.overwrite_request_body(Bytes::from_static(b"12345")),
        Err(BodyReadError::BufferLimitExceeded { limit: 4 })
    );
    assert_eq!(
        over.request_body.read_all().await.unwrap().unwrap(),
        Bytes::from_static(b"old"),
        "a rejected mutation must leave the previous version visible"
    );
}

#[tokio::test]
async fn slice_replacement_enforces_physical_cap_atomically() {
    let mut ctx = context_with_body(b"abcd", 5).await;
    assert_eq!(
        ctx.replace_request_body_slice(1..3, Bytes::from_static(b"WXYZ")).await,
        Err(BodyReadError::BufferLimitExceeded { limit: 5 })
    );
    assert_eq!(
        ctx.request_body.read_all().await.unwrap().unwrap(),
        Bytes::from_static(b"abcd")
    );
}
