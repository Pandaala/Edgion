//! Real socket compatibility check for Pingora's unbuffered Tokio stream seam.
//! Owns one loopback connection; no Gateway/Controller topology is started.

use pingora_core::protocols::l4::stream::Stream;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn pingora_tcp_stream_constructs_and_transfers_both_directions() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut stream = Stream::from(socket);
            let mut received = [0; 4];
            stream.read_exact(&mut received).await.unwrap();
            assert_eq!(&received, b"ping");
            stream.write_all(b"pong").await.unwrap();
            stream.flush().await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut received = [0; 4];
        client.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"pong");
        server.await.unwrap();
    })
    .await
    .expect("Pingora TCP exchange exceeded its deadline");
}
