use axum::body::Body;
use axum::extract::Extension;
use axum::extract::{ConnectInfo, Request};
use axum::response::IntoResponse;
use axum::{routing::get, Router};
use futures::future;
use std::net::SocketAddr;
use tokio::task;

async fn hello(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(server_addr): Extension<String>,
    req: Request<Body>,
) -> impl IntoResponse {
    let mut resp = String::with_capacity(1024);
    resp.push_str(&format!(
        "\n\n============= Response from {} ===========\n",
        server_addr
    ));

    let headers = req.headers();

    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    resp.push_str(&format!("Host: {}\n", host));

    let path = req.uri().path();
    resp.push_str(&format!("Path: {}\n", path));
    resp.push_str(&format!("Client Address: {}\n", addr));
    resp.push_str(&format!("Client Port: {}\n", addr.port()));

    resp.push_str("\nHeaders:\n");
    for (key, value) in headers {
        resp.push_str(&format!(
            "  {}: {}\n",
            key,
            value.to_str().unwrap_or("<invalid utf8>")
        ));
    }

    resp.push_str("\n");
    resp
}

#[tokio::main]
async fn main() {
    println!("Starting test servers...");
    let app1 = Router::new()
        .route("/{*path}", get(hello))
        .layer(Extension("127.0.0.1:30001".to_string()));
    let app2 = Router::new()
        .route("/{*path}", get(hello))
        .layer(Extension("127.0.0.1:30002".to_string()));
    let app3 = Router::new()
        .route("/{*path}", get(hello))
        .layer(Extension("127.0.0.1:30003".to_string()));
    let app4 = Router::new()
        .route("/{*path}", get(hello))
        .layer(Extension("127.0.0.1:30004".to_string()));

    let addrs = [
        SocketAddr::from("127.0.0.1:30001".parse::<SocketAddr>().unwrap()),
        SocketAddr::from("127.0.0.1:30002".parse::<SocketAddr>().unwrap()),
        SocketAddr::from("127.0.0.1:30003".parse::<SocketAddr>().unwrap()),
        SocketAddr::from("127.0.0.1:30004".parse::<SocketAddr>().unwrap()),
    ];

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
    println!("Listening on {}", addr);

    // Add ConnectInfo layer to enable client address extraction
    let app_with_connect_info = app.into_make_service_with_connect_info::<SocketAddr>();

    axum::serve(listener, app_with_connect_info).await.unwrap();
}
