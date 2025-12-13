use axum::{
    extract::{ws::WebSocket, ConnectInfo, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Extension, Router,
};
use futures::{future, SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::task;

/// WebSocket handler
async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(server_addr): Extension<String>,
) -> impl IntoResponse {
    println!("[{}] New WebSocket connection from {}", server_addr, addr);
    ws.on_upgrade(move |socket| handle_socket(socket, addr, server_addr))
}

/// Handle individual WebSocket connection
async fn handle_socket(socket: WebSocket, client_addr: SocketAddr, server_addr: String) {
    let (mut sender, mut receiver) = socket.split();

    // Send welcome message
    let welcome_msg = format!(
        "Connected to WebSocket server {} from {}",
        server_addr, client_addr
    );
    if sender.send(axum::extract::ws::Message::Text(welcome_msg.into())).await.is_err() {
        println!("[{}] Failed to send welcome message to {}", server_addr, client_addr);
        return;
    }

    // Echo messages back to client
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(axum::extract::ws::Message::Text(text)) => {
                println!("[{}] Received text from {}: {}", server_addr, client_addr, text);
                
                // Echo back with server info
                let response = format!("[{}] Echo: {}", server_addr, text);
                if sender
                    .send(axum::extract::ws::Message::Text(response.into()))
                    .await
                    .is_err()
                {
                    println!("[{}] Client {} disconnected", server_addr, client_addr);
                    break;
                }
            }
            Ok(axum::extract::ws::Message::Binary(data)) => {
                println!("[{}] Received {} bytes from {}", server_addr, data.len(), client_addr);
                
                // Echo back binary data
                if sender
                    .send(axum::extract::ws::Message::Binary(data))
                    .await
                    .is_err()
                {
                    println!("[{}] Client {} disconnected", server_addr, client_addr);
                    break;
                }
            }
            Ok(axum::extract::ws::Message::Ping(data)) => {
                println!("[{}] Received ping from {}", server_addr, client_addr);
                if sender
                    .send(axum::extract::ws::Message::Pong(data))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(axum::extract::ws::Message::Pong(_)) => {
                println!("[{}] Received pong from {}", server_addr, client_addr);
            }
            Ok(axum::extract::ws::Message::Close(_)) => {
                println!("[{}] Client {} closed connection", server_addr, client_addr);
                break;
            }
            Err(e) => {
                println!("[{}] WebSocket error from {}: {}", server_addr, client_addr, e);
                break;
            }
        }
    }

    println!("[{}] Connection closed for {}", server_addr, client_addr);
}

#[tokio::main]
async fn main() {
    println!("Starting WebSocket test servers...\n");

    // Create 4 WebSocket servers on different ports
    let app1 = Router::new()
        .route("/ws", get(ws_handler))
        .layer(Extension("127.0.0.1:30011".to_string()));
    
    let app2 = Router::new()
        .route("/ws", get(ws_handler))
        .layer(Extension("127.0.0.1:30012".to_string()));
    
    let app3 = Router::new()
        .route("/ws", get(ws_handler))
        .layer(Extension("127.0.0.1:30013".to_string()));
    
    let app4 = Router::new()
        .route("/ws", get(ws_handler))
        .layer(Extension("127.0.0.1:30014".to_string()));

    let addrs = [
        "127.0.0.1:30011".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:30012".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:30013".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:30014".parse::<SocketAddr>().unwrap(),
    ];

    println!("WebSocket servers will listen on:");
    for addr in &addrs {
        println!("  - ws://{}/ws", addr);
    }
    println!();

    let servers = [
        task::spawn(run_server(addrs[0], app1)),
        task::spawn(run_server(addrs[1], app2)),
        task::spawn(run_server(addrs[2], app3)),
        task::spawn(run_server(addrs[3], app4)),
    ];

    future::join_all(servers).await;
}

async fn run_server(addr: SocketAddr, app: Router) {
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("WebSocket server listening on ws://{}/ws", addr);

    let app_with_connect_info = app.into_make_service_with_connect_info::<SocketAddr>();

    axum::serve(listener, app_with_connect_info).await.unwrap();
}
