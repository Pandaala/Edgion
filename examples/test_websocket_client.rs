use tokio_tungstenite::{connect_async, tungstenite::protocol::Message, tungstenite::handshake::client::Request};
use futures::{SinkExt, StreamExt};
use std::time::Duration;

#[tokio::main]
async fn main() {
    println!("WebSocket Test Client\n");

    // Test servers (assuming they're running on ports 30011-30014)
    let servers = vec![
        "ws://127.0.0.1:30011/ws",
        "ws://127.0.0.1:30012/ws",
        "ws://127.0.0.1:30013/ws",
        "ws://127.0.0.1:30014/ws",
    ];

    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    let server_url_input = if args.len() > 1 {
        args[1].clone()
    } else {
        println!("Usage: {} <ws://server:port/path> [host_header]", args[0]);
        println!("Example: {} ws://127.0.0.1:8080/ws example.com", args[0]);
        println!("Using default: {}\n", servers[0]);
        servers[0].to_string()
    };

    // Optional host header from command line (second argument)
    let custom_host = if args.len() > 2 {
        Some(args[2].clone())
    } else {
        None
    };

    // Convert http:// to ws:// if needed
    let server_url = if server_url_input.starts_with("http://") {
        server_url_input.replace("http://", "ws://")
    } else if server_url_input.starts_with("https://") {
        server_url_input.replace("https://", "wss://")
    } else {
        server_url_input
    };

    if let Some(ref host) = custom_host {
        println!("Connecting to {} with Host: {}...", server_url, host);
    } else {
        println!("Connecting to {}...", server_url);
    }

    // Build WebSocket request with custom Host header if provided
    let connect_result = if let Some(host) = custom_host {
        // Build custom request with Host header
        let request = Request::builder()
            .uri(&server_url)
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
            .body(())
            .expect("Failed to build request");
        
        connect_async(request).await
    } else {
        connect_async(server_url).await
    };

    match connect_result {
        Ok((ws_stream, _)) => {
            println!("✓ Connected successfully!\n");
            
            let (mut write, mut read) = ws_stream.split();

            // Spawn a task to send messages
            let send_task = tokio::spawn(async move {
                for i in 1..=5 {
                    let msg = format!("Hello from client, message #{}", i);
                    println!("→ Sending: {}", msg);
                    
                    if write.send(Message::Text(msg)).await.is_err() {
                        println!("✗ Failed to send message");
                        break;
                    }
                    
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }

                // Send a ping
                println!("→ Sending: PING");
                if write.send(Message::Ping(vec![1, 2, 3])).await.is_ok() {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }

                // Send binary data
                println!("→ Sending: Binary data [5 bytes]");
                if write.send(Message::Binary(vec![1, 2, 3, 4, 5])).await.is_ok() {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }

                // Close connection
                println!("→ Closing connection");
                let _ = write.send(Message::Close(None)).await;
            });

            // Receive messages
            let recv_task = tokio::spawn(async move {
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            println!("← Received: {}", text);
                        }
                        Ok(Message::Binary(data)) => {
                            println!("← Received: Binary data [{} bytes]", data.len());
                        }
                        Ok(Message::Ping(_)) => {
                            println!("← Received: PING");
                        }
                        Ok(Message::Pong(_)) => {
                            println!("← Received: PONG");
                        }
                        Ok(Message::Close(_)) => {
                            println!("← Connection closed by server");
                            break;
                        }
                        Ok(Message::Frame(_)) => {
                            // Raw frames are not typically seen
                        }
                        Err(e) => {
                            println!("✗ Error receiving message: {}", e);
                            break;
                        }
                    }
                }
            });

            // Wait for both tasks to complete
            let _ = tokio::join!(send_task, recv_task);
            
            println!("\n✓ Test completed");
        }
        Err(e) => {
            println!("✗ Failed to connect: {}", e);
            println!("\nMake sure the WebSocket server is running:");
            println!("  cargo run --example test_websocket_server");
        }
    }
}
