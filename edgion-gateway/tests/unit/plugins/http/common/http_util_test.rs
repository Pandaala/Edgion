use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
struct Doc {
    name: String,
}

async fn spawn_server_with_body(body: Vec<u8>, send_content_length: bool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut throwaway = [0u8; 1024];
        let _ = sock.read(&mut throwaway).await;
        let mut header = String::from("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n");
        if send_content_length {
            header.push_str(&format!("Content-Length: {}\r\n", body.len()));
        } else {
            header.push_str(&format!("Transfer-Encoding: chunked\r\n\r\n{:x}\r\n", body.len()));
        }
        if send_content_length {
            header.push_str("\r\n");
        }
        let _ = sock.write_all(header.as_bytes()).await;
        let _ = sock.write_all(&body).await;
        if !send_content_length {
            let _ = sock.write_all(b"\r\n0\r\n\r\n").await;
        }
        let _ = sock.shutdown().await;
    });
    format!("http://{}/", addr)
}

#[tokio::test]
async fn ok_within_limit() {
    let url = spawn_server_with_body(br#"{"name":"alice"}"#.to_vec(), true).await;
    let resp = reqwest::get(&url).await.unwrap();
    let doc: Doc = fetch_json_limited(resp, 1024, "test").await.unwrap();
    assert_eq!(doc, Doc { name: "alice".into() });
}

#[tokio::test]
async fn content_length_rejects_early() {
    // 4096 bytes of body advertised via Content-Length, but limit is 128.
    let body = vec![b'x'; 4096];
    let url = spawn_server_with_body(body, true).await;
    let resp = reqwest::get(&url).await.unwrap();
    let err = fetch_json_limited::<Doc>(resp, 128, "test").await.unwrap_err();
    assert!(err.contains("Content-Length"), "got: {}", err);
}

#[tokio::test]
async fn stream_rejects_dishonest_chunked() {
    // No Content-Length (chunked) — must be caught by streaming accumulator.
    let body = vec![b'y'; 4096];
    let url = spawn_server_with_body(body, false).await;
    let resp = reqwest::get(&url).await.unwrap();
    let err = fetch_json_limited::<Doc>(resp, 128, "test").await.unwrap_err();
    assert!(err.contains("exceeds limit"), "got: {}", err);
}

#[tokio::test]
async fn parse_error_surfaces_with_ctx() {
    let url = spawn_server_with_body(b"not json".to_vec(), true).await;
    let resp = reqwest::get(&url).await.unwrap();
    let err = fetch_json_limited::<Doc>(resp, 1024, "testctx").await.unwrap_err();
    assert!(err.contains("Failed to parse testctx"), "got: {}", err);
}
